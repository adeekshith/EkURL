use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use ekurl::{create_router, AppState, Db, ShortenResponse};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tower::util::ServiceExt;

#[tokio::test]
async fn test_db_operations() -> anyhow::Result<()> {
    // Use in-memory DB for testing
    let db = Db::new(":memory:").await?;

    // 1. Insert (with expiry far in future)
    let code = "test1".to_string();
    let url = "https://example.com".to_string();
    let future_ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64 + 86400;
    let inserted = db.insert(code.clone(), url.clone(), Some(future_ts)).await?;
    assert!(inserted, "Should insert new URL");

    // 2. Get
    let retrieved = db.get_url(code.clone()).await?;
    assert_eq!(retrieved, Some(url.clone()));

    // 3. Exists
    let exists = db.exists(code.clone()).await?;
    assert!(exists);

    // 4. Count
    let count = db.count().await?;
    assert_eq!(count, 1);

    // 5. List
    let list = db.list().await?;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].0, code);
    assert_eq!(list[0].1, url);
    assert_eq!(list[0].2, Some(future_ts));

    // 6. Delete
    let deleted = db.delete(code.clone()).await?;
    assert!(deleted, "Should delete existing URL");

    let count_after = db.count().await?;
    assert_eq!(count_after, 0);

    let deleted_again = db.delete(code.clone()).await?;
    assert!(!deleted_again, "Should return false for non-existent URL");

    Ok(())
}

#[tokio::test]
async fn test_db_expiry() -> anyhow::Result<()> {
    let db = Db::new(":memory:").await?;

    // Insert an already-expired URL
    let past_ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64 - 10;
    db.insert("expired".to_string(), "https://example.com".to_string(), Some(past_ts)).await?;

    // Should not be retrievable
    assert_eq!(db.get_url("expired".to_string()).await?, None);
    assert!(!db.exists("expired".to_string()).await?);
    assert_eq!(db.count().await?, 0);
    assert!(db.list().await?.is_empty());

    // Insert a never-expiring URL
    db.insert("forever".to_string(), "https://example.com".to_string(), None).await?;
    assert_eq!(db.get_url("forever".to_string()).await?, Some("https://example.com".to_string()));
    assert!(db.exists("forever".to_string()).await?);
    assert_eq!(db.count().await?, 1);

    // Cleanup should remove the expired one
    let cleaned = db.cleanup_expired().await?;
    assert_eq!(cleaned, 1);

    // The forever one should still be there
    assert_eq!(db.count().await?, 1);

    Ok(())
}

#[tokio::test]
async fn test_api_shorten() -> anyhow::Result<()> {
    let db = Db::new(":memory:").await?;
    let state = Arc::new(AppState { db });
    let app = create_router(state);

    // 1. Valid Shorten (default expiry = 1d)
    let response = app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/shorten")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"url": "https://rust-lang.org"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    let json: ShortenResponse = serde_json::from_slice(&body)?;
    // Fresh DB -> first auto-generated code should be the minimum length (3).
    assert_eq!(json.code.len(), 3);
    // Auto-generated codes must only use lowercase letters and digits.
    assert!(json.code.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
    // Default expiry should be ~7 days from now
    assert!(json.expires_at.is_some());
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let diff = json.expires_at.unwrap() - now;
    const SEVEN_DAYS: i64 = 7 * 86400;
    assert!((SEVEN_DAYS - 5..=SEVEN_DAYS).contains(&diff));

    // 2. Invalid URL
    let response = app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/shorten")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"url": "not-a-url"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // 3. Custom Code
    let response = app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/shorten")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"url": "https://google.com", "custom_code": "google"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    // 4. Duplicate Custom Code
    let response = app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/shorten")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"url": "https://other.com", "custom_code": "google"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);

    // 5. Same Domain Restriction
    let response = app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/shorten")
                .header("Content-Type", "application/json")
                .header("Host", "myapp.com")
                .body(Body::from(r#"{"url": "https://myapp.com/foo"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // 6. Never-expiring URL
    let response = app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/shorten")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"url": "https://forever.com", "expires_in": "never"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
    let json: ShortenResponse = serde_json::from_slice(&body)?;
    assert!(json.expires_at.is_none());

    // 7. Invalid expires_in value
    let response = app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/shorten")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"url": "https://example.com", "expires_in": "5m"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // 8. Removed expires_in values (30m, 1h) are now rejected
    for removed in ["30m", "1h"] {
        let body = format!(r#"{{"url": "https://example.com", "expires_in": "{}"}}"#, removed);
        let response = app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/shorten")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "expires_in='{}' should be rejected", removed);
    }

    // 9. New long-duration expires_in values produce roughly correct timestamps
    const DAY: i64 = 86400;
    let cases = [("1mo", 30 * DAY), ("3mo", 90 * DAY), ("6mo", 180 * DAY), ("1y", 365 * DAY)];
    for (input, expected_secs) in cases {
        let body = format!(
            r#"{{"url": "https://example.com/{}", "expires_in": "{}"}}"#,
            input, input
        );
        let response = app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/shorten")
                    .header("Content-Type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED, "expires_in='{}' should succeed", input);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await?;
        let json: ShortenResponse = serde_json::from_slice(&body)?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
        let diff = json.expires_at.expect("should have expiry") - now;
        assert!(
            (expected_secs - 5..=expected_secs).contains(&diff),
            "expires_in='{}' produced diff={}, expected ~{}", input, diff, expected_secs
        );
    }

    Ok(())
}

#[tokio::test]
async fn test_api_redirect() -> anyhow::Result<()> {
    let db = Db::new(":memory:").await?;
    // Insert with future expiry
    let future_ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64 + 86400;
    db.insert("rust".to_string(), "https://rust-lang.org".to_string(), Some(future_ts)).await?;

    // Insert an expired URL
    let past_ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64 - 10;
    db.insert("old".to_string(), "https://old.com".to_string(), Some(past_ts)).await?;

    let state = Arc::new(AppState { db });
    let app = create_router(state);

    // 1. Found (not expired)
    let response = app.clone()
        .oneshot(
            Request::builder()
                .uri("/rust")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(response.headers().get("location").unwrap(), "https://rust-lang.org");

    // 2. Not Found
    let response = app.clone()
        .oneshot(
            Request::builder()
                .uri("/unknown")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // 3. Expired URL returns 404
    let response = app.clone()
        .oneshot(
            Request::builder()
                .uri("/old")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    Ok(())
}
