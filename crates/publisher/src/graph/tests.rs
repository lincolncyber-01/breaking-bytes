use super::*;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path, query_param},
};

fn client(server: &MockServer) -> Result<Graph, Box<dyn std::error::Error>> {
    Ok(Graph::build(
        Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        Url::parse(&format!("{}/v27.0/", server.uri()))?,
        "123",
        "test-secret",
    )?)
}

#[tokio::test]
async fn official_story_contract_and_sensitive_errors() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v27.0/123/content_publishing_limit"))
        .and(query_param("fields", "quota_usage,config"))
        .and(header("authorization", "Bearer test-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data":[{"quota_usage":49,"config":{"quota_total":50,"quota_duration":86400}}]}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let graph = client(&server)?;
    assert_eq!(
        graph.quota().await?,
        Quota {
            usage: 49,
            total: 50
        }
    );
    Mock::given(method("POST"))
        .and(path("/v27.0/123/media"))
        .and(body_json(
            json!({"image_url":"https://example.com/story.jpg","media_type":"STORIES"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"456"})))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(graph.create("https://example.com/story.jpg").await?, "456");
    for state in ["IN_PROGRESS", "FINISHED", "PUBLISHED", "ERROR", "EXPIRED"] {
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/v27.0/456"))
            .and(query_param("fields", "status_code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status_code":state})))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            serde_json::to_value(state)?,
            json!(match graph.status("456").await? {
                ContainerStatus::InProgress => "IN_PROGRESS",
                ContainerStatus::Finished => "FINISHED",
                ContainerStatus::Published => "PUBLISHED",
                ContainerStatus::Error => "ERROR",
                ContainerStatus::Expired => "EXPIRED",
            })
        );
    }
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/v27.0/123/media_publish"))
        .and(body_json(json!({"creation_id":"456"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"789"})))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(graph.publish("456").await?, "789");
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_string("test-secret https://secret.invalid/?token=secret"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let error = graph.publish("456").await.err().ok_or("erro esperado")?;
    assert!(!format!("{error:?} {error}").contains("secret"));
    server.verify().await;
    Ok(())
}

#[tokio::test]
async fn pagination_never_follows_remote_next_url() -> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let server = MockServer::start().await;
    let graph = client(&server)?;
    Mock::given(path("/v27.0/123/stories")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":"1"}],"paging":{"next":"http://127.0.0.1/private","cursors":{"after":"cursor-one"}}}))).with_priority(10).expect(1).mount(&server).await;
    Mock::given(path("/v27.0/123/stories"))
        .and(query_param("after", "cursor-one"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":"2"}]})))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(graph.stories().await?, vec!["1", "2"]);
    server.verify().await;
    Ok(())
}

#[tokio::test]
async fn missing_quota_unknown_status_size_and_ids_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let server = MockServer::start().await;
    let graph = client(&server)?;
    assert!(graph.status("../123").await.is_err());
    assert!(graph.publish("https://secret.invalid").await.is_err());
    assert!(graph.create("file:///etc/passwd").await.is_err());
    for response in [
        json!({"data":[{"quota_usage":0}]}),
        json!({"data":[{"quota_usage":0,"config":{"quota_total":100,"quota_duration":3600}}]}),
        json!({"data":[]}),
    ] {
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&server)
            .await;
        assert!(graph.quota().await.is_err());
    }
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status_code":"UNKNOWN"})))
        .mount(&server)
        .await;
    assert!(graph.status("456").await.is_err());
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(MAX_JSON + 1)))
        .mount(&server)
        .await;
    assert!(graph.quota().await.is_err());
    assert!(
        Graph::new(
            Client::new(),
            Login::Facebook,
            "v27.0/other",
            "123",
            "token"
        )
        .is_err()
    );
    assert!(Graph::new(Client::new(), Login::Instagram, "v27.0", "123/456", "token").is_err());
    Ok(())
}

#[tokio::test]
async fn oauth_and_permission_codes_have_sanitized_typed_errors()
-> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let server = MockServer::start().await;
    let graph = client(&server)?;
    for code in [102, 190, 10, 200, 299] {
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(json!({"error":{"code":code,"message":"fixture-secret"}})),
            )
            .mount(&server)
            .await;
        let error = graph.quota().await.err().ok_or("erro esperado")?;
        assert!(error.token_failure().is_some());
        assert!(!format!("{error:?}").contains("fixture-secret"));
    }
    Ok(())
}


#[tokio::test]
async fn retry_after_preserves_graph_code_and_never_exposes_error_message()->Result<(),Box<dyn std::error::Error>> {
    let server=MockServer::start().await;
    Mock::given(path("/v27.0/123/content_publishing_limit")).respond_with(ResponseTemplate::new(429).insert_header("retry-after","120").set_body_json(json!({"error":{"code":4,"message":"credential-must-not-escape"}}))).mount(&server).await;
    let graph=client(&server)?;
    let error=graph.quota().await.err().ok_or("erro esperado")?;
    assert_eq!(error.retry_after(),Some(120));assert_eq!(error.code(),4);
    assert!(!format!("{error:?} {error}").contains("credential-must-not-escape"));
    Ok(())
}
