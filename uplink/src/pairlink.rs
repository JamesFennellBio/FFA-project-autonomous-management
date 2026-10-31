//! What pairing QR codes carry, and drawing them.
//!
//! - Hub pairing (printed by `hub-node pair`): `farmlab-hub://<ip>:7402?c=<12 digits>`
//! - Phone pairing (shown by the desktop app):
//!   `farmlab://pair?v=2&a=<hub ip:7302>&k=<hub KeyId hex>&t=<token hex>&e=<unix expiry>`
//!
//! CRYPTO-REVIEW: the phone pins `k` (the hub's KeyId), so it only ever pairs
//! with that hub, and proves it saw this QR with `t` (see
//! crypto_sharecrate::crypto::v2::pairing::verify_ticket). The token is single
//! use and expires on the hub after `PHONE_TICKET_TTL_SECS`.

use anyhow::{Context, Result, bail};
use crypto_sharecrate::crypto::v2::identity::KeyId;
use crypto_sharecrate::crypto::v2::pairing::TICKET_TOKEN_LEN;
use qrcode::QrCode;
use qrcode::render::{svg, unicode};

/// How long a phone pairing QR code works.
pub const PHONE_TICKET_TTL_SECS: u32 = 300;

/// The contents of a phone pairing QR code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhoneQr {
    /// Hub address the phone connects to, "ip:port".
    pub addr: String,
    pub hub_key_id: KeyId,
    pub token: [u8; TICKET_TOKEN_LEN],
    /// Unix time after which the hub refuses the token (shown to the person).
    pub expires: i64,
}

impl PhoneQr {
    pub fn to_uri(&self) -> String {
        format!(
            "farmlab://pair?v=2&a={}&k={}&t={}&e={}",
            self.addr,
            hex::encode(self.hub_key_id),
            hex::encode(self.token),
            self.expires
        )
    }

    pub fn parse(uri: &str) -> Result<Self> {
        let q = uri
            .trim()
            .strip_prefix("farmlab://pair?")
            .context("not a Farm & Lab pairing code")?;
        let get = |name: &str| {
            q.split('&')
                .find_map(|kv| kv.strip_prefix(name).and_then(|v| v.strip_prefix('=')))
                .with_context(|| format!("pairing code is missing `{name}`"))
        };
        if get("v")? != "2" {
            bail!("this pairing code is for a different app version");
        }
        let addr = get("a")?.to_string();
        if addr.is_empty() || addr.len() > 64 || !addr.contains(':') {
            bail!("pairing code has a bad hub address");
        }
        let hub_key_id =
            crate::key_from_hex(get("k")?).context("pairing code has a bad hub key")?;
        let t = hex::decode(get("t")?).context("pairing code has a bad token")?;
        let token: [u8; TICKET_TOKEN_LEN] = t
            .try_into()
            .map_err(|_| anyhow::anyhow!("pairing code has a bad token"))?;
        let expires = get("e")?.parse().context("pairing code has a bad expiry")?;
        Ok(Self {
            addr,
            hub_key_id,
            token,
            expires,
        })
    }
}

/// The hub pairing code as a QR payload.
pub fn hub_code_uri(addr: &str, code: &str) -> String {
    format!("farmlab-hub://{addr}?c={}", code.replace('-', ""))
}

/// A QR code as a standalone SVG (black on white, so phone cameras read it in
/// either app theme).
pub fn qr_svg(text: &str) -> Result<String> {
    let code = QrCode::new(text.as_bytes())?;
    Ok(code
        .render::<svg::Color<'_>>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

/// A QR code drawn with half-block characters for a terminal (SSH session).
pub fn qr_terminal(text: &str) -> Result<String> {
    let code = QrCode::new(text.as_bytes())?;
    Ok(code
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_qr_roundtrip_and_rejects_junk() {
        let q = PhoneQr {
            addr: "192.168.1.40:7302".into(),
            hub_key_id: [7u8; 32],
            token: [9u8; 16],
            expires: 1_800_000_000,
        };
        assert_eq!(PhoneQr::parse(&q.to_uri()).unwrap(), q);
        assert!(PhoneQr::parse("https://example.com").is_err());
        assert!(PhoneQr::parse(&q.to_uri().replace("v=2", "v=1")).is_err());
        assert!(PhoneQr::parse(&q.to_uri().replace("&t=", "&t=00")).is_err());
    }

    #[test]
    fn qr_renders() {
        assert!(qr_svg("farmlab://pair?v=2").unwrap().contains("<svg"));
        assert!(!qr_terminal("x").unwrap().is_empty());
    }
}
