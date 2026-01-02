use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use ekurl::{create_router, AppState, Db, ShortenResponse};
use std::sync::Arc;
use tower::util::ServiceExt;

#[tokio::test]
async fn test_db_operations() -> anyhow::Result<()> {
    // Use in-memory DB for testing
    let db = Db::new(":memory:").await?;

    // 1. Insert
    let code = "test1".to_string();
    let url = "https://example.com".to_string();
    let inserted = db.insert(code.clone(), url.clone()).await?;
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
    assert_eq!(list[0], (code.clone(), url.clone()));

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
async fn test_api_shorten() -> anyhow::Result<()> {
    let db = Db::new(":memory:").await?;
    let state = Arc::new(AppState { db });
    let app = create_router(state);

    // 1. Valid Shorten
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
    assert!(!json.code.is_empty());

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

    Ok(())
}

#[tokio::test]
async fn test_api_redirect() -> anyhow::Result<()> {
    let db = Db::new(":memory:").await?;
    db.insert("rust".to_string(), "https://rust-lang.org".to_string()).await?;
    
    let state = Arc::new(AppState { db });
    let app = create_router(state);

    // 1. Found
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

    Ok(())
}
