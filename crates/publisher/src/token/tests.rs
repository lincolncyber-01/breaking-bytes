use super::*;
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path, query_param},
};

fn client(server: &MockServer, login: Login) -> Result<TokenClient, Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    Ok(TokenClient {
        http: Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        base: Url::parse(&format!("{}/", server.uri()))?,
        login,
    })
}
fn facebook(now: DateTime<Utc>) -> Value {
    json!({"data":{"app_id":"99","type":"USER","is_valid":true,"issued_at":(now-Duration::days(30)).timestamp(),"expires_at":(now+Duration::days(30)).timestamp(),"data_access_expires_at":0,"scopes":["instagram_basic","instagram_content_publish","pages_read_engagement"]}})
}

#[tokio::test]
async fn inspection_checks_app_permissions_page_and_both_expirations()
-> Result<(), Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    let now = Utc::now();
    let client = client(&server, Login::Facebook)?;
    let token = Credential::new("fixture-user-secret")?;
    let inspector = Credential::new("fixture-inspector-secret")?;
    Mock::given(method("GET"))
        .and(path("/debug_token"))
        .and(query_param("input_token", "fixture-user-secret"))
        .and(header("authorization", "Bearer fixture-inspector-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(facebook(now)))
        .expect(1)
        .mount(&server)
        .await;
    let claims = client
        .inspect_facebook(&token, &inspector, "99", None, now)
        .await?;
    assert!(claims.publication_allowed(now, 300));
    assert!(!claims.refresh_due(now, 30));
    server.verify().await;
    for (case, expected) in [
        (0, TokenFailure::WrongApp),
        (1, TokenFailure::Permission),
        (2, TokenFailure::Invalid),
        (3, TokenFailure::WrongAccount),
        (4, TokenFailure::Invalid),
    ] {
        server.reset().await;
        let mut response = facebook(now);
        match case {
            0 => response["data"]["app_id"] = json!("100"),
            1 => response["data"]["scopes"] = json!(["instagram_basic"]),
            2 => response["data"]["expires_at"] = json!(now.timestamp()),
            3 => {
                response["data"]["type"] = json!("PAGE");
                response["data"]["profile_id"] = json!("123");
            }
            _ => response["data"]["data_access_expires_at"] = json!(now.timestamp()),
        }
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&server)
            .await;
        assert_eq!(
            client
                .inspect_facebook(&token, &inspector, "99", None, now)
                .await
                .err()
                .ok_or("erro esperado")?
                .failure(),
            expected
        );
    }
    server.reset().await;
    let mut page = facebook(now);
    page["data"]["type"] = json!("PAGE");
    page["data"]["profile_id"] = json!("123");
    page["data"]["expires_at"] = json!(0);
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page))
        .mount(&server)
        .await;
    let claims = client
        .inspect_facebook(&token, &inspector, "99", Some("123"), now)
        .await?;
    assert_eq!(claims.kind, TokenKind::FacebookPage);
    assert!(claims.expires_at.is_none());
    assert!(claims.publication_allowed(now, 300));
    Ok(())
}

#[tokio::test]
async fn instagram_refresh_never_exposes_replacement_or_error_bodies()
-> Result<(), Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    let now = Utc::now();
    let client = client(&server, Login::Instagram)?;
    let token = Credential::new("fixture-current-secret")?;
    for replacement in ["fixture-current-secret", "fixture-replacement-secret"] {
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/refresh_access_token"))
            .and(query_param("grant_type", "ig_refresh_token"))
            .and(query_param("access_token", "fixture-current-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"access_token":replacement,"token_type":"bearer","expires_in":5183944}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let outcome = client.refresh_instagram(&token, now).await?;
        assert!(!format!("{outcome:?}").contains("secret"));
        assert_eq!(
            matches!(outcome, RefreshOutcome::RotationRequired),
            replacement != "fixture-current-secret"
        );
        server.verify().await;
    }
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            json!({"error":{"message":"fixture-current-secret fixture-replacement-secret"}}),
        ))
        .mount(&server)
        .await;
    let error = client
        .refresh_instagram(&token, now)
        .await
        .err()
        .ok_or("erro esperado")?;
    assert!(!format!("{error:?} {error}").contains("secret"));
    assert!(!format!("{token:?}").contains("secret"));
    assert!(
        client
            .inspect_facebook(&token, &token, "99", None, now)
            .await
            .is_err()
    );
    Ok(())
}

#[test]
fn oauth_receipt_is_bound_to_credential_and_long_lived_window()
-> Result<(), Box<dyn std::error::Error>> {
    let now = Utc::now();
    let token = Credential::new("fixture-current-secret")?;
    let receipt = json!({"fingerprint":blake3::Hash::from_bytes(token.fingerprint()).to_hex().to_string(),"issued_at":now-Duration::days(30),"expires_at":now+Duration::days(30),"scopes":["instagram_business_basic","instagram_business_content_publish"]});
    let mut extras = receipt.clone();
    extras["scopes"]
        .as_array_mut()
        .ok_or("scopes ausentes")?
        .push(json!("fixture-current-secret"));
    assert!(
        !format!("{:?}", instagram_receipt(&extras.to_string(), &token, now)?)
            .contains("fixture-current-secret")
    );
    let claims = instagram_receipt(&receipt.to_string(), &token, now)?;
    assert!(claims.refresh_due(now, 30));
    let mut invalid = receipt.clone();
    invalid["fingerprint"] = json!("0".repeat(64));
    assert!(instagram_receipt(&invalid.to_string(), &token, now).is_err());
    invalid = receipt.clone();
    invalid["scopes"] = json!(["instagram_business_basic"]);
    assert!(instagram_receipt(&invalid.to_string(), &token, now).is_err());
    invalid = receipt.clone();
    invalid["access_token"] = json!("not-allowed");
    assert!(instagram_receipt(&invalid.to_string(), &token, now).is_err());
    invalid = receipt;
    invalid["expires_at"] = json!(now + Duration::days(40));
    assert!(instagram_receipt(&invalid.to_string(), &token, now).is_err());
    Ok(())
}

#[tokio::test]
async fn oauth_response_limits_malformed_data_and_timeout_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let server = MockServer::start().await;
    let now = Utc::now();
    let client = client(&server, Login::Instagram)?;
    let token = Credential::new("fixture-current-secret")?;
    for body in [
        "fixture-secret-not-json".into(),
        "x".repeat(64 * 1024 + 1),
        json!({"access_token":"fixture-secret","token_type":"bearer","expires_in":1}).to_string(),
    ] {
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;
        let error = client
            .refresh_instagram(&token, now)
            .await
            .err()
            .ok_or("erro esperado")?;
        assert!(!format!("{error:?} {error}").contains("fixture-secret"));
        server.verify().await;
    }
    server.reset().await;
    Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(11)).set_body_json(json!({"access_token":"fixture-current-secret","token_type":"bearer","expires_in":5183944}))).expect(1).mount(&server).await;
    assert!(matches!(
        client.refresh_instagram(&token, now).await,
        Err(TokenError::Api(PublishError::Transport))
    ));
    server.verify().await;
    assert!(Credential::new("secret\nheader").is_err());
    Ok(())
}
