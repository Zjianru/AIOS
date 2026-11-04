// T006.F2 红测试：/connect 换 cookie 与 DoD7 六种情况（骨架先行；签名实现待依赖批准）。

use web::{
    ConnectGrant, Trust, TrustError, authorize_api_request, authorize_connect, check_api_auth,
    check_origin,
};

fn trust() -> Trust {
    Trust::new(
        std::net::IpAddr::from([127, 0, 0, 1]),
        1417,
        "tok-1".to_owned(),
        [7u8; 32],
    )
}

fn headers(host: &str) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("host", host.parse().unwrap());
    headers
}

fn mint_with(trust: &Trust) -> ConnectGrant {
    authorize_connect(trust, Some("127.0.0.1:1417"), Some("tok-1"))
        .expect("正确令牌必须能换到 cookie")
}

fn mint() -> ConnectGrant {
    mint_with(&trust())
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
    // 正确 cookie 名 + 坏 value，确保走到 HMAC 验签，而不是名字先被拒。
    let grant = mint();
    let pair = cookie_pair(&grant);
    let (name, _value) = pair.split_once('=').expect("cookie 必须有名字");
    let mut headers = headers("127.0.0.1:1417");
    headers.insert("cookie", format!("{name}=v1.bad.bad").parse().unwrap());
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
fn dod7_token_in_api_or_ws_query_is_rejected() {
    // /api?token= 与 /ws?token= 攻击面：带有效 cookie 也要 401，证明 query token 不被接受。
    let trust = trust();
    let grant = mint_with(&trust);
    let mut headers = headers("127.0.0.1:1417");
    headers.insert("cookie", cookie_pair(&grant).parse().unwrap());
    for uri in ["/api?token=tok-1", "/ws?token=tok-1"] {
        let uri: axum::http::Uri = uri.parse().unwrap();
        let error = authorize_api_request(&trust, &headers, &uri).unwrap_err();
        assert_eq!(error.status(), 401, "URI {uri} 必须拒绝 query token");
    }
    let plain: axum::http::Uri = "/api".parse().unwrap();
    assert!(authorize_api_request(&trust, &headers, &plain).is_ok());
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

#[test]
fn rotated_signing_key_invalidates_previous_cookie() {
    // T008.F11：签名钥匙来自 Keyring，删记录/重新开始后下次启动换新 → 旧页面会话失效。
    let old = Trust::new(
        std::net::IpAddr::from([127, 0, 0, 1]),
        1417,
        "tok-1".to_owned(),
        [1u8; 32],
    );
    let same = Trust::new(
        std::net::IpAddr::from([127, 0, 0, 1]),
        1417,
        "tok-1".to_owned(),
        [1u8; 32],
    );
    let rotated = Trust::new(
        std::net::IpAddr::from([127, 0, 0, 1]),
        1417,
        "tok-1".to_owned(),
        [2u8; 32],
    );
    let grant = mint_with(&old);
    let mut headers = headers("127.0.0.1:1417");
    headers.insert("cookie", cookie_pair(&grant).parse().unwrap());
    assert!(
        check_api_auth(&same, &headers).is_ok(),
        "同一把钥匙（Keyring 记录未删）重启后旧 cookie 仍有效"
    );
    let error = check_api_auth(&rotated, &headers).unwrap_err();
    assert_eq!(error.status(), 401, "换钥匙后旧 cookie 必须 401");
}
