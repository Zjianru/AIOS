//! web 集成测试共用：loopback Trust 与 Host 头。

use axum::http::HeaderMap;
use web::Trust;

pub fn trust() -> Trust {
    Trust::new(
        std::net::IpAddr::from([127, 0, 0, 1]),
        1417,
        "tok-1".to_owned(),
        [7u8; 32],
    )
}

pub fn headers(host: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("host", host.parse().unwrap());
    headers
}
