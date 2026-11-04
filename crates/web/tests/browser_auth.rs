// T006.F2 红测试：/connect 换 cookie 与 DoD7 六种情况（骨架先行；签名实现待依赖批准）。

use web::{ConnectGrant, Trust, TrustError, authorize_connect, check_api_auth, check_origin};

fn trust() -> Trust {
    Trust::new(
        std::net::IpAddr::from([127, 0, 0, 1]),
        1417,
        "tok-1".to_owned(),
    )
}

fn headers(host: &str) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("host", host.parse().unwrap());
    headers
}

fn mint() -> ConnectGrant {
    authorize_connect(&trust(), Some("127.0.0.1:1417"), Some("tok-1"))
        .expect("正确令牌必须能换到 cookie")
}

fn cookie_pair(grant: &ConnectGrant) -> String {
    grant
        .set_cookie
        .split(';')
        .next()
        .expect("Set-Cookie 不能为空")
        .to_owned()
}

#[test]
fn dod7_connect_mints_hardened_cookie() {
    let grant = mint();
    for attribute in [
        "HttpOnly",
        "SameSite=Strict",
        "Path=/",
        "Max-Age=",
        "Expires=",
    ] {
        assert!(
            grant.set_cookie.contains(attribute),
            "Set-Cookie 缺少 {attribute}: {}",
            grant.set_cookie
        );
    }
    assert!(
        grant.set_cookie.starts_with("aios-auth-"),
        "cookie 名要按 authority 绑定：{}",
        grant.set_cookie
    );
    assert!(!grant.location.is_empty(), "换 cookie 后要有跳转地址");
}

#[test]
fn dod7_connect_rejects_missing_or_wrong_token() {
    let trust = trust();
    for token in [None, Some("tok-2")] {
        let error = authorize_connect(&trust, Some("127.0.0.1:1417"), token).unwrap_err();
        assert_eq!(error.status(), 401);
        assert!(matches!(error, TrustError::Unauthorized));
    }
}

#[test]
fn dod7_no_cookie_is_401() {
    let error = check_api_auth(&trust(), &headers("127.0.0.1:1417")).unwrap_err();
    assert_eq!(error.status(), 401);
}

#[test]
fn dod7_wrong_signature_cookie_is_401() {
    let mut headers = headers("127.0.0.1:1417");
    headers.insert("cookie", "aios-auth-x=v1.bad.bad".parse().unwrap());
    let error = check_api_auth(&trust(), &headers).unwrap_err();
    assert_eq!(error.status(), 401);
}

#[test]
fn dod7_cookie_bound_to_other_authority_is_401() {
    let cookie = cookie_pair(&mint());
    let mut headers = headers("127.0.0.1:1418");
    headers.insert("cookie", cookie.parse().unwrap());
    let error = check_api_auth(&trust(), &headers).unwrap_err();
    assert_eq!(error.status(), 401);
}

#[test]
fn dod7_token_outside_connect_is_rejected() {
    // /api、/ws 不认 query token：没有 cookie 就只有 401（handler 接线后 query 会被忽略）。
    let mut headers = headers("127.0.0.1:1417");
    headers.insert("x-token", "tok-1".parse().unwrap());
    let error = check_api_auth(&trust(), &headers).unwrap_err();
    assert_eq!(error.status(), 401);
}

#[test]
fn dod7_cross_site_origin_is_403() {
    let mut headers = headers("127.0.0.1:1417");
    headers.insert("origin", "https://evil.example".parse().unwrap());
    let error = check_origin(&headers).unwrap_err();
    assert_eq!(error.status(), 403);
}

#[test]
fn dod7_sec_fetch_cross_site_is_403() {
    let mut headers = headers("127.0.0.1:1417");
    headers.insert("sec-fetch-site", "cross-site".parse().unwrap());
    let error = check_origin(&headers).unwrap_err();
    assert_eq!(error.status(), 403);
}
