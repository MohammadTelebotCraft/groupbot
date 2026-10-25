use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use reqwest::header::{ACCEPT_ENCODING, CONTENT_ENCODING};

const FETCH_DEADLINE: Duration = Duration::from_secs(8);
const MAX_BYTES: usize = 16 * 1024 * 1024;

pub(super) async fn fetch_no_proxy(url: &str, declared_size: i32) -> Result<Vec<u8>, &'static str> {
    tokio::time::timeout(FETCH_DEADLINE, fetch_inner(url, declared_size))
        .await
        .map_err(|_| "invoice photo fetch timed out")?
}

async fn fetch_inner(url: &str, declared_size: i32) -> Result<Vec<u8>, &'static str> {
    let (url, domain, expected) = validate_url(url, declared_size)?;
    let resolved = tokio::net::lookup_host((domain.as_str(), 443))
        .await
        .map_err(|_| "invoice photo DNS failed")?;
    let addresses = validate_addresses(resolved)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .timeout(FETCH_DEADLINE)
        .resolve_to_addrs(&domain, &addresses)
        .build()
        .map_err(|_| "invoice photo HTTP client failed")?;
    let mut response = client
        .get(url)
        .header(ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|_| "invoice photo HTTP request failed")?;
    if response.status() != reqwest::StatusCode::OK {
        return Err("invoice photo HTTP status was not 200");
    }
    if response
        .headers()
        .get(CONTENT_ENCODING)
        .is_some_and(|value| !value.as_bytes().eq_ignore_ascii_case(b"identity"))
    {
        return Err("invoice photo HTTP response was encoded");
    }
    if response
        .content_length()
        .is_some_and(|size| size != expected as u64)
    {
        return Err("invoice photo HTTP length differs from Telegram metadata");
    }

    let mut bytes = Vec::with_capacity(expected);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "invoice photo HTTP body failed")?
    {
        if bytes.len().saturating_add(chunk.len()) > expected {
            return Err("invoice photo exceeded its declared size");
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() != expected {
        return Err("invoice photo ended before its declared size");
    }
    Ok(bytes)
}

fn validate_url(
    raw: &str,
    declared_size: i32,
) -> Result<(reqwest::Url, String, usize), &'static str> {
    let expected = usize::try_from(declared_size).map_err(|_| "invoice photo size is invalid")?;
    if expected == 0 || expected > MAX_BYTES {
        return Err("invoice photo size is outside the byte limit");
    }
    let url = reqwest::Url::parse(raw).map_err(|_| "invoice photo URL is invalid")?;
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("invoice photo URL is not a plain HTTPS URL");
    }
    let domain = url
        .domain()
        .filter(|host| host.contains('.') && !host.ends_with('.'))
        .ok_or("invoice photo URL has no DNS domain")?
        .to_owned();
    Ok((url, domain, expected))
}

fn validate_addresses(
    resolved: impl IntoIterator<Item = SocketAddr>,
) -> Result<Vec<SocketAddr>, &'static str> {
    let mut addresses = Vec::new();
    for address in resolved {
        if address.port() != 443 || !public_ip(address.ip()) {
            return Err("invoice photo DNS returned a nonpublic address");
        }
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    if addresses.is_empty() {
        return Err("invoice photo DNS returned no address");
    }
    Ok(addresses)
}

fn in_v4(ip: Ipv4Addr, base: Ipv4Addr, prefix: u32) -> bool {
    u32::from(ip) >> (32 - prefix) == u32::from(base) >> (32 - prefix)
}

fn in_v6(ip: Ipv6Addr, base: Ipv6Addr, prefix: u32) -> bool {
    u128::from(ip) >> (128 - prefix) == u128::from(base) >> (128 - prefix)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ![
                (Ipv4Addr::new(0, 0, 0, 0), 8),
                (Ipv4Addr::new(10, 0, 0, 0), 8),
                (Ipv4Addr::new(100, 64, 0, 0), 10),
                (Ipv4Addr::new(127, 0, 0, 0), 8),
                (Ipv4Addr::new(169, 254, 0, 0), 16),
                (Ipv4Addr::new(172, 16, 0, 0), 12),
                (Ipv4Addr::new(192, 0, 0, 0), 24),
                (Ipv4Addr::new(192, 0, 2, 0), 24),
                (Ipv4Addr::new(192, 88, 99, 0), 24),
                (Ipv4Addr::new(192, 168, 0, 0), 16),
                (Ipv4Addr::new(198, 18, 0, 0), 15),
                (Ipv4Addr::new(198, 51, 100, 0), 24),
                (Ipv4Addr::new(203, 0, 113, 0), 24),
                (Ipv4Addr::new(224, 0, 0, 0), 4),
                (Ipv4Addr::new(240, 0, 0, 0), 4),
            ]
            .into_iter()
            .any(|(base, prefix)| in_v4(ip, base, prefix))
        }
        IpAddr::V6(ip) => {
            in_v6(ip, Ipv6Addr::new(0x2000, 0, 0, 0, 0, 0, 0, 0), 3)
                && ![
                    (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23),
                    (Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0), 32),
                    (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16),
                    (Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20),
                ]
                .into_iter()
                .any(|(base, prefix)| in_v6(ip, base, prefix))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invoice_photo_url_requires_plain_https_dns_and_bounded_size() {
        assert!(validate_url("https://cdn.example.com/item.jpg?token=abc", 5).is_ok());
        for url in [
            "http://cdn.example.com/item.jpg",
            "https://cdn.example.com:8443/item.jpg",
            "https://user:pass@cdn.example.com/item.jpg",
            "https://127.0.0.1/item.jpg",
            "https://[::1]/item.jpg",
            "https://cdn.example.com./item.jpg",
            "https://cdn.example.com/item.jpg#fragment",
        ] {
            assert!(validate_url(url, 5).is_err(), "{url}");
        }
        assert!(validate_url("https://cdn.example.com/item.jpg", 0).is_err());
        assert!(validate_url("https://cdn.example.com/item.jpg", -1).is_err());
        assert!(validate_url("https://cdn.example.com/item.jpg", MAX_BYTES as i32 + 1).is_err());
    }

    #[test]
    fn invoice_photo_dns_accepts_only_public_pinned_addresses() {
        let public = [
            SocketAddr::from(([8, 8, 8, 8], 443)),
            SocketAddr::new("2606:4700:4700::1111".parse().unwrap(), 443),
        ];
        assert_eq!(validate_addresses(public).unwrap(), public);
        for ip in [
            "0.1.2.3",
            "10.1.2.3",
            "100.64.1.2",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.0.2.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "2002:c0a8:101::1",
            "3fff::1",
        ] {
            let address = SocketAddr::new(ip.parse().unwrap(), 443);
            assert!(validate_addresses([address]).is_err(), "{ip}");
            assert!(validate_addresses([public[0], address]).is_err(), "{ip}");
        }
        assert!(validate_addresses([SocketAddr::from(([8, 8, 8, 8], 80))]).is_err());
        assert!(validate_addresses(Vec::<SocketAddr>::new()).is_err());
    }
}
