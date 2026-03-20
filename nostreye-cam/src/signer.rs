use anyhow::{Context, Result};
use device_signer::identity::DeviceIdentity;
use nostr::{EventId, PublicKey, Tag};
use sha2::{Digest, Sha256};
use tracing::info;


pub struct NostreYeSigner {
    identity: DeviceIdentity,
}

impl NostreYeSigner {
    pub fn new(camera_id: Option<String>) -> Result<Self> {
        let identity =
            DeviceIdentity::new(camera_id).context("Failed to initialise DeviceIdentity")?;
        info!("DeviceIdentity initialised");
        info!("  Nostr pubkey (hex) : {}", identity.info.nostr_pubkey_hex);
        info!("  npub               : {}", identity.info.nostr_npub);
        info!("  Ethereum address   : {}", identity.info.eth_address);
        Ok(Self { identity })
    }

    /// Return the device's Nostr public key in `npub1…` bech32 format.
    pub fn npub(&self) -> &str {
        &self.identity.info.nostr_npub
    }

    /// Return the device's Nostr public key as a lowercase hex string.
    pub fn pubkey_hex(&self) -> &str {
        &self.identity.info.nostr_pubkey_hex
    }

    pub fn sign_text_note(&self, content: &str, extra_tags: Vec<Tag>) -> Result<SignedEvent> {
        //  Build the serialised event commitment (NIP-01 §4) 
        let created_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let pubkey_hex = &self.identity.info.nostr_pubkey_hex;

        // Serialise tags to JSON array
        let tags_json: Vec<serde_json::Value> = extra_tags
            .iter()
            .map(|t| {

                let tag_str = format!("{:?}", t); 
                serde_json::Value::Array(vec![serde_json::Value::String(tag_str)])
            })
            .collect();

        let commitment = serde_json::json!([
            0,
            pubkey_hex,
            created_at,
            1,        
            tags_json,
            content,
        ]);
        let commitment_str = serde_json::to_string(&commitment)?;

        // Compute event ID = SHA-256(commitment) 
        let mut hasher = Sha256::new();
        hasher.update(commitment_str.as_bytes());
        let event_id_bytes: [u8; 32] = hasher.finalize().into();
        let event_id_hex = hex::encode(event_id_bytes);

        // Sign with device Schnorr key 
        let sig_hex = self
            .identity
            .sign_nostr_event(&event_id_bytes)
            .map_err(|e| anyhow::anyhow!("Schnorr signing failed: {:?}", e))?;

        info!("Signed Nostr event");
        info!("  event_id : {}", event_id_hex);
        info!("  sig      : {}…", &sig_hex[..16]);

        let event_json = serde_json::json!({
            "id":         event_id_hex,
            "pubkey":     pubkey_hex,
            "created_at": created_at,
            "kind":       1,
            "tags":       serde_json::Value::Array(vec![]),
            "content":    content,
            "sig":        sig_hex,
        });

        Ok(SignedEvent {
            id: event_id_hex,
            pubkey: pubkey_hex.clone(),
            created_at,
            kind: 1,
            content: content.to_string(),
            sig: sig_hex,
            json: serde_json::to_string_pretty(&event_json)?,
        })
    }

    /// Sign an ECDSA hash (e.g. a 32-byte image hash) and return the hex
    pub fn sign_frame_hash(&self, frame_data: &[u8]) -> Result<String> {
        // SHA-256 of the frame bytes
        let mut hasher = Sha256::new();
        hasher.update(frame_data);
        let hash: [u8; 32] = hasher.finalize().into();

        let sig = self
            .identity
            .sign_hash_ecdsa(&hash)
            .map_err(|e| anyhow::anyhow!("ECDSA signing failed: {:?}", e))?;

        info!(
            "Frame ECDSA signature: {}…",
            &sig[..16.min(sig.len())]
        );
        Ok(sig)
    }

    /// Verify that a [`SignedEvent`]'s `sig` was produced by its claimed
    pub fn verify_event(event: &SignedEvent) -> Result<bool> {
        // Recompute event ID commitment
        let commitment = serde_json::json!([
            0,
            event.pubkey,
            event.created_at,
            event.kind,
            serde_json::Value::Array(vec![]),
            event.content,
        ]);
        let commitment_str = serde_json::to_string(&commitment)?;
        let mut hasher = Sha256::new();
        hasher.update(commitment_str.as_bytes());
        let computed_id: [u8; 32] = hasher.finalize().into();
        let computed_id_hex = hex::encode(computed_id);

        if computed_id_hex != event.id {
            return Ok(false);
        }

        // Use nostr crate to verify Schnorr signature
        let pubkey = PublicKey::from_hex(&event.pubkey)
            .map_err(|e| anyhow::anyhow!("Invalid pubkey: {}", e))?;
        let event_id = EventId::from_hex(&event.id)
            .map_err(|e| anyhow::anyhow!("Invalid event id: {}", e))?;

        let sig_bytes = hex::decode(&event.sig).context("Invalid signature hex")?;
        let schnorr_sig = nostr::secp256k1::schnorr::Signature::from_slice(&sig_bytes)
            .map_err(|e| anyhow::anyhow!("Invalid signature bytes: {}", e))?;

        let secp = nostr::secp256k1::Secp256k1::new();
        let msg = nostr::secp256k1::Message::from_digest(*event_id.as_bytes());
        let xonly = nostr::secp256k1::XOnlyPublicKey::from_slice(pubkey.as_bytes())
            .unwrap_or_else(|_| nostr::secp256k1::XOnlyPublicKey::from_slice(&pubkey.to_bytes()).unwrap());

        match secp.verify_schnorr(&schnorr_sig, &msg, &xonly) {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SignedEvent {
    pub id: String,
    pub pubkey: String,
    pub created_at: u64,
    pub kind: u64,
    pub content: String,
    pub sig: String,
    pub json: String,
}
