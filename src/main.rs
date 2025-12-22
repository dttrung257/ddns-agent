use anyhow::{anyhow, Context};
use reqwest::Client;
use serde::Deserialize;
use std::{env, net::Ipv4Addr};
use tokio::time::{sleep, Duration};

#[derive(Deserialize)]
struct CfResponse {
    success: bool,
}

#[derive(Deserialize)]
struct CfZonesResponse {
    success: bool,
    result: Vec<CfZone>,
}

#[derive(Deserialize)]
struct CfZone {
    id: String,
}

#[derive(Deserialize)]
struct CfDnsRecordsResponse {
    success: bool,
    result: Vec<CfDnsRecord>,
}

#[derive(Deserialize)]
struct CfDnsRecord {
    id: String,
}

/// Extract root domain from DNS name (e.g., "sub.example.com" -> "example.com")
fn extract_root_domain(dns_name: &str) -> String {
    let parts: Vec<&str> = dns_name.split('.').collect();
    if parts.len() >= 2 {
        parts[parts.len() - 2..].join(".")
    } else {
        dns_name.to_string()
    }
}

/// Fetch Zone ID from Cloudflare API based on domain name
async fn get_zone_id(client: &Client, cf_token: &str, dns_name: &str) -> anyhow::Result<String> {
    let domain = extract_root_domain(dns_name);
    let url = format!("https://api.cloudflare.com/client/v4/zones?name={}", domain);
    let resp = client
        .get(&url)
        .bearer_auth(cf_token)
        .send()
        .await
        .context("Failed to fetch zones from Cloudflare")?;

    let data: CfZonesResponse = resp.json().await.context("Failed to parse zones response")?;

    if !data.success || data.result.is_empty() {
        return Err(anyhow!("Zone not found for domain: {}", domain));
    }

    Ok(data.result[0].id.clone())
}

/// Fetch DNS Record ID from Cloudflare API based on zone_id and dns_name
async fn get_record_id(
    client: &Client,
    cf_token: &str,
    zone_id: &str,
    dns_name: &str,
) -> anyhow::Result<String> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/zones/{}/dns_records?type=A&name={}",
        zone_id, dns_name
    );

    let resp = client
        .get(&url)
        .bearer_auth(cf_token)
        .send()
        .await
        .context("Failed to fetch DNS records from Cloudflare")?;

    let data: CfDnsRecordsResponse = resp
        .json()
        .await
        .context("Failed to parse DNS records response")?;

    if !data.success || data.result.is_empty() {
        return Err(anyhow!("DNS record not found for: {}", dns_name));
    }

    Ok(data.result[0].id.clone())
}

#[inline]
pub async fn get_public_ip() -> anyhow::Result<Option<Ipv4Addr>> {
    let public_ip = public_ip::addr_v4().await;

    Ok(public_ip)
}

async fn update_dns(
    client: &Client,
    ip: &str,
    cf_token: &str,
    zone_id: &str,
    record_id: &str,
    dns_name: &str,
) -> anyhow::Result<()> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/zones/{}/dns_records/{}",
        zone_id, record_id
    );

    let body = serde_json::json!({
        "type": "A",
        "name": dns_name,
        "content": ip,
        "ttl": 1, // 1 for auto
        "proxied": false
    });

    let resp = client
        .put(url)
        .bearer_auth(cf_token)
        .json(&body)
        .send()
        .await?;

    let data: CfResponse = resp.json().await?;
    if data.success {
        println!("[OK] DNS updated: {}", ip);
    } else {
        eprintln!("[ERR] Failed to update DNS");
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let client = Client::new();
    let mut last_ip = String::new();
    let duration_sleep_ms: u64 = env::var("DURATION_SLEEP_MS")
        .unwrap_or_else(|_| "5000".to_string())
        .parse()
        .unwrap_or(5000);

    // Get required env vars
    let cf_token = env::var("CF_API_TOKEN").context("CF_API_TOKEN is required")?;
    let dns_name = env::var("DNS_NAME").context("DNS_NAME is required")?;

    // Fetch Zone ID and Record ID dynamically from Cloudflare API
    println!("[INFO] Fetching Zone ID for: {}", dns_name);
    let zone_id = get_zone_id(&client, &cf_token, &dns_name).await?;
    println!("[INFO] Zone ID: {}", zone_id);

    println!("[INFO] Fetching Record ID for: {}", dns_name);
    let record_id = get_record_id(&client, &cf_token, &zone_id, &dns_name).await?;
    println!("[INFO] Record ID: {}", record_id);

    println!("[INFO] Starting IP sync loop...");
    loop {
        match get_public_ip().await {
            Ok(Some(ip)) => {
                let ip_str = ip.to_string();
                if ip_str != last_ip {
                    println!("[INFO] New IP: {}", ip_str);
                    if let Err(e) = update_dns(&client, &ip_str, &cf_token, &zone_id, &record_id, &dns_name).await {
                        eprintln!("[ERR] {}", e);
                    } else {
                        last_ip = ip_str;
                    }
                }
            }
            Ok(None) => eprintln!("[ERR] Could not determine public IP"),
            Err(e) => eprintln!("[ERR] {}", e),
        }

        sleep(Duration::from_millis(duration_sleep_ms)).await;
    }
}
