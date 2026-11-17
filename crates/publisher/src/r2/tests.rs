use super::*;

#[tokio::test]
async fn sigv4_is_bound_to_r2_without_network_or_default_credentials()
-> Result<(), Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let r2 = R2::new(
        Client::new(),
        &"a".repeat(32),
        "stories-img",
        "TESTACCESS",
        "test-secret",
        Duration::from_secs(3600),
    )?;
    let url = r2.get_url("stories/1/story.jpg").await?;
    assert!(!format!("{url:?}").contains("TESTACCESS"));
    let parsed = url::Url::parse(url.expose())?;
    assert_eq!(
        parsed.host_str(),
        Some(format!("{}.r2.cloudflarestorage.com", "a".repeat(32)).as_str())
    );
    assert_eq!(parsed.path(), "/stories-img/stories/1/story.jpg");
    let pairs = parsed
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(
        pairs.get("X-Amz-Algorithm").map(|v| v.as_ref()),
        Some("AWS4-HMAC-SHA256")
    );
    assert_eq!(pairs.get("X-Amz-Expires").map(|v| v.as_ref()), Some("3600"));
    assert!(
        pairs
            .get("X-Amz-Credential")
            .is_some_and(|v| v.ends_with("/auto/s3/aws4_request"))
    );
    assert!(pairs.contains_key("X-Amz-Signature"));
    assert!(r2.get_url("../secret").await.is_err());
    assert!(
        r2.put_verified("story.jpg", vec![1], [0; 32])
            .await
            .is_err()
    );
    assert!(
        R2::new(
            Client::new(),
            "https://127.0.0.1/",
            "stories-img",
            "key",
            "secret",
            Duration::from_secs(3600)
        )
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn signed_put_readback_and_idempotent_delete_contract()
-> Result<(), Box<dyn std::error::Error>> {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path, query_param},
    };
    let _ = rustls::crypto::ring::default_provider().install_default();
    let server = MockServer::start().await;
    let mut r2 = R2::new(
        Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        &"a".repeat(32),
        "stories-img",
        "TESTACCESS",
        "test-secret",
        Duration::from_secs(3600),
    )?;
    r2.signer = Signer::from_conf(
        r2.signer
            .config()
            .to_builder()
            .endpoint_url(server.uri())
            .build(),
    );
    let bytes = vec![0xff, 0xd8, 0xff, 0xd9];
    let hash = *blake3::hash(&bytes).as_bytes();
    Mock::given(method("PUT"))
        .and(path("/stories-img/story.jpg"))
        .and(header("content-type", "image/jpeg"))
        .and(query_param("X-Amz-Algorithm", "AWS4-HMAC-SHA256"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/stories-img/story.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
        .expect(1)
        .mount(&server)
        .await;
    r2.put_verified("story.jpg", bytes.clone(), hash).await?;
    Mock::given(method("DELETE"))
        .and(path("/stories-img/story.jpg"))
        .respond_with(ResponseTemplate::new(404))
        .expect(2)
        .mount(&server)
        .await;
    r2.delete("story.jpg").await?;
    r2.delete("story.jpg").await?;
    server.verify().await;
    server.reset().await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1, 2, 3]))
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        r2.put_verified("story.jpg", bytes, hash).await,
        Err(PublishError::Artifact)
    ));
    server.verify().await;
    Ok(())
}
