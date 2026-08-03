use hickory_resolver::config::{NameServerConfig, Protocol, ResolverConfig, ResolverOpts};
use hickory_resolver::proto::rr::RData;
use hickory_resolver::proto::rr::RecordType;
use hickory_resolver::TokioAsyncResolver;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

/// 自定义 DNS 解析器，使用可信 DNS 服务器（绕过本地 DNS 污染）
#[derive(Clone)]
pub struct TrustedDns {
    resolver: TokioAsyncResolver,
}

impl TrustedDns {
    pub fn new() -> Self {
        let mut config = ResolverConfig::new();
        // Cloudflare
        config.add_name_server(NameServerConfig::new(
            "1.1.1.1:53".parse().unwrap(),
            Protocol::Udp,
        ));
        config.add_name_server(NameServerConfig::new(
            "1.0.0.1:53".parse().unwrap(),
            Protocol::Udp,
        ));
        // Google
        config.add_name_server(NameServerConfig::new(
            "8.8.8.8:53".parse().unwrap(),
            Protocol::Udp,
        ));
        config.add_name_server(NameServerConfig::new(
            "8.8.4.4:53".parse().unwrap(),
            Protocol::Udp,
        ));

        let mut opts = ResolverOpts::default();
        opts.timeout = std::time::Duration::from_secs(5);
        opts.attempts = 3;
        opts.validate = false;

        let resolver = TokioAsyncResolver::tokio(config, opts);
        Self { resolver }
    }
}

impl Resolve for TrustedDns {
    fn resolve(&self, name: Name) -> Resolving {
        let resolver = self.resolver.clone();
        Box::pin(async move {
            let name_str = name.as_str();

            // 如果已经是 IP 地址，直接返回
            if let Ok(ip) = name_str.parse::<std::net::IpAddr>() {
                let addrs: Addrs =
                    Box::new(vec![SocketAddr::new(ip, 443), SocketAddr::new(ip, 80)].into_iter());
                return Ok(addrs);
            }

            let mut results = Vec::new();

            // 查询 A 记录 (IPv4)
            if let Ok(response) = resolver.lookup(name_str, RecordType::A).await {
                for record in response.record_iter() {
                    if let Some(RData::A(a)) = record.data() {
                        results.push(SocketAddr::new(IpAddr::V4((*a).into()), 443));
                    }
                }
            }

            // 查询 AAAA 记录 (IPv6)
            if let Ok(response) = resolver.lookup(name_str, RecordType::AAAA).await {
                for record in response.record_iter() {
                    if let Some(RData::AAAA(aaaa)) = record.data() {
                        results.push(SocketAddr::new(IpAddr::V6((*aaaa).into()), 443));
                    }
                }
            }

            if results.is_empty() {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("DNS resolution failed for {name_str}"),
                ))
                    as Box<dyn std::error::Error + Send + Sync>);
            }

            let addrs: Addrs = Box::new(results.into_iter());
            Ok(addrs)
        })
    }
}

/// 创建带自定义 DNS 和可选出站代理的 reqwest Client。
pub fn build_client(
    timeout_secs: u64,
    connect_timeout_secs: u64,
    proxy_url: Option<&str>,
) -> Result<reqwest::Client, String> {
    let dns = Arc::new(TrustedDns::new());
    let mut builder = reqwest::Client::builder()
        .dns_resolver(dns)
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .connect_timeout(std::time::Duration::from_secs(connect_timeout_secs));
    if let Some(proxy_url) = proxy_url {
        let proxy =
            reqwest::Proxy::all(proxy_url).map_err(|error| format!("出站代理地址无效: {error}"))?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|error| format!("初始化 HTTP 客户端失败: {error}"))
}
