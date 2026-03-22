use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use futures_util::{SinkExt, StreamExt};
use sha2::Digest;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn};

use crate::signer::{NostreyeSigner, SignedEvent};

/// Public Blossom server and Nostr relays used by default.
pub const BLOSSOM_SERVER: &str = "https://blossom.band";
pub const RELAYS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://nos.lol",
    "wss://relay.primal.net",
    "wss://relay.snort.social",
    "wss://relay.nostr.band",
    "wss://nostr.mom",
];

pub struct PublishResult {
    pub image_url: String,
    /// Per-relay result: (relay_url, accepted)
    pub relay_results: Vec<(String, bool)>,
}

/// Broadcast a signed event to all relays. Returns per-relay (url, accepted).
pub async fn broadcast_event(event: &SignedEvent, relays: &[&str]) -> Vec<(String, bool)> {
    let mut results = Vec::new();
    for &relay in relays {
        match publish_to_relay(event, relay).await {
            Ok(accepted) => {
                info!("Relay {} → {}", relay, if accepted { "accepted" } else { "rejected" });
                results.push((relay.to_string(), accepted));
            }
            Err(e) => {
                warn!("Relay {} error: {:#}", relay, e);
                results.push((relay.to_string(), false));
            }
        }
    }
    results
}

/// Upload `jpeg_data` to Blossom, sign a NIP-94 (kind 1063) event with the
/// image URL + ECDSA attestation, then broadcast to `relays`.
pub async fn publish_image(
    jpeg_data: &[u8],
    ecdsa_sig: &str,
    width: u32,
    height: u32,
    signer: &NostreyeSigner,
    blossom_server: &str,
    relays: &[&str],
) -> Result<PublishResult> {
    // SHA-256 of the file — used for `x` tag and Blossom auth
    let sha256_hex = {
        let mut h = sha2::Sha256::new();
        h.update(jpeg_data);
        hex::encode(h.finalize())
    };

    // Build Blossom auth event (kind 24242) and upload
    let auth_json = signer.sign_blossom_auth(&sha256_hex, jpeg_data.len() as u64)?;
    let auth_b64 = B64.encode(auth_json.as_bytes());

    info!("Uploading {} bytes to {}", jpeg_data.len(), blossom_server);
    let image_url = upload_to_blossom(jpeg_data, &auth_b64, blossom_server).await?;
    info!("Blossom upload OK → {}", image_url);

    // Build NIP-94 (kind 1063) event
    let content = format!(
        "📷 nostreye capture — ECDSA attestation: {}…",
        &ecdsa_sig[..32.min(ecdsa_sig.len())]
    );
    let tags = vec![
        vec!["url".to_string(), image_url.clone()],
        vec!["m".to_string(), "image/jpeg".to_string()],
        vec!["x".to_string(), sha256_hex],
        vec!["size".to_string(), jpeg_data.len().to_string()],
        vec!["dim".to_string(), format!("{}x{}", width, height)],
        vec!["ecdsa-attestation".to_string(), ecdsa_sig.to_string()],
    ];
    let nip94 = signer.sign_event(1063, &content, tags)?;
    info!("NIP-94 event signed: {}", nip94.id);

    // Kind 1 with image URL — visible in normal clients (Damus, Primal, Snort)
    let note_content = format!("📷 nostreye capture\n\n{}", image_url);
    let note = signer.sign_event(1, &note_content, vec![])?;
    info!("Kind 1 image post signed: {}", note.id);

    // Broadcast both kind 1 and kind 1063
    let r1 = broadcast_event(&note, relays).await;
    let r2 = broadcast_event(&nip94, relays).await;
    // Use kind 1 results for display (both should match; take first)
    let relay_results = if !r1.is_empty() { r1 } else { r2 };

    Ok(PublishResult { image_url, relay_results })
}

/// PUT the JPEG bytes to a Blossom server and return the URL.
async fn upload_to_blossom(data: &[u8], auth_b64: &str, server: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    let url = format!("{}/upload", server.trim_end_matches('/'));
    let resp = client
        .put(&url)
        .header("Authorization", format!("Nostr {}", auth_b64))
        .header("Content-Type", "image/jpeg")
        .body(data.to_vec())
        .send()
        .await
        .context("Blossom PUT request failed")?;

    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .context("Blossom response was not JSON")?;

    if !status.is_success() {
        return Err(anyhow!("Blossom HTTP {}: {}", status, body));
    }

    body["url"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("No 'url' field in blossom response: {}", body))
}

/// Connect to a Nostr relay via WebSocket, send the event, and wait for an OK.
async fn publish_to_relay(event: &SignedEvent, relay_url: &str) -> Result<bool> {
    let event_obj: serde_json::Value =
        serde_json::from_str(&event.json).context("Failed to parse event JSON")?;
    let wire = serde_json::to_string(&serde_json::json!(["EVENT", event_obj]))?;

    let (mut ws, _) = connect_async(relay_url)
        .await
        .with_context(|| format!("WebSocket connect to {} failed", relay_url))?;

    ws.send(Message::Text(wire)).await.context("WS send failed")?;

    let accepted = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(frame) = ws.next().await {
            let text = match frame.context("WS read error")? {
                Message::Text(t) => t,
                Message::Close(_) => break,
                _ => continue,
            };
            if let Ok(serde_json::Value::Array(fields)) = serde_json::from_str(&text) {
                if fields.first().and_then(|v| v.as_str()) == Some("OK") {
                    return Ok(fields.get(2).and_then(|v| v.as_bool()).unwrap_or(false));
                }
            }
        }
        Err(anyhow!("Relay closed without OK"))
    })
    .await
    .context("Relay response timed out")??;

    ws.close(None).await.ok();
    Ok(accepted)
}
