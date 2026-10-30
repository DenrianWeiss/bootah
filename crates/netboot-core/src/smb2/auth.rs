use hmac::{Hmac, Mac};
use md4::{Digest, Md4};
use md5::Md5;
use rc4::{KeyInit, Rc4, StreamCipher, consts::U16};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use super::{Credential, get16, get32, get64, now, utf16, wide};

const NTLM_OID: &[u8] = &[0x2b, 6, 1, 4, 1, 0x82, 0x37, 2, 2, 10];
const UNICODE: u32 = 1;
const SIGN: u32 = 0x10;
const NTLM: u32 = 0x200;
const EXTENDED: u32 = 0x80000;
const VERSION: u32 = 0x02000000;
const KEY_EXCH: u32 = 0x40000000;

fn md5(key: &[u8], data: &[u8]) -> [u8; 16] {
    let mut mac = <Hmac<Md5> as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}
pub(super) fn signature(key: &[u8; 16], message: &[u8]) -> [u8; 16] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(&message[..48]);
    mac.update(&[0; 16]);
    mac.update(&message[64..]);
    mac.finalize().into_bytes()[..16].try_into().unwrap()
}
pub(super) fn equal(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}
fn der(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    if value.len() < 128 {
        out.push(value.len() as u8);
    } else if value.len() < 256 {
        out.extend([0x81, value.len() as u8]);
    } else {
        out.push(0x82);
        out.extend((value.len() as u16).to_be_bytes());
    }
    out.extend(value);
    out
}
pub(super) fn initial_token() -> Vec<u8> {
    let mechanisms = der(0xa0, &der(0x30, &der(6, NTLM_OID)));
    let mut token = der(6, &[0x2b, 6, 1, 5, 5, 2]);
    token.extend(der(0xa0, &der(0x30, &mechanisms)));
    der(0x60, &token)
}
fn response_token(token: Option<&[u8]>) -> Vec<u8> {
    let mut inner = der(0xa0, &der(0x0a, &[u8::from(token.is_some())]));
    if let Some(token) = token {
        inner.extend(der(0xa1, &der(6, NTLM_OID)));
        inner.extend(der(0xa2, &der(4, token)));
    }
    der(0xa1, &der(0x30, &inner))
}
pub(super) fn completed_token() -> Vec<u8> {
    response_token(None)
}
fn ntlm_token(data: &[u8], depth: usize) -> Option<&[u8]> {
    if data.starts_with(b"NTLMSSP\0") {
        return Some(data);
    }
    if depth > 8 || data.len() > 16384 {
        return None;
    }
    let mut rest = data;
    while !rest.is_empty() {
        let tag = *rest.first()?;
        let first = *rest.get(1)?;
        let (len, head) = if first < 128 {
            (first as usize, 2)
        } else {
            let count = (first & 127) as usize;
            if count == 0 || count > 2 {
                return None;
            }
            let mut len = 0usize;
            for byte in rest.get(2..2 + count)? {
                len = (len << 8) | *byte as usize;
            }
            (len, 2 + count)
        };
        let value = rest.get(head..head + len)?;
        if (tag & 0x20 != 0 || tag == 4)
            && let Some(token) = ntlm_token(value, depth + 1)
        {
            return Some(token);
        }
        rest = rest.get(head + len..)?;
    }
    None
}
fn security_buffer(data: &[u8], at: usize) -> Option<&[u8]> {
    let len = get16(data, at).ok()? as usize;
    let allocated = get16(data, at + 2).ok()? as usize;
    if len > allocated {
        return None;
    }
    let offset = get32(data, at + 4).ok()? as usize;
    data.get(offset..offset.checked_add(len)?)
}
fn write_buffer(out: &mut [u8], at: usize, len: usize, offset: usize) {
    out[at..at + 2].copy_from_slice(&(len as u16).to_le_bytes());
    out[at + 2..at + 4].copy_from_slice(&(len as u16).to_le_bytes());
    out[at + 4..at + 8].copy_from_slice(&(offset as u32).to_le_bytes());
}
fn av(out: &mut Vec<u8>, id: u16, value: &[u8]) {
    out.extend(id.to_le_bytes());
    out.extend((value.len() as u16).to_le_bytes());
    out.extend(value);
}

pub(super) struct Challenge {
    negotiate: Vec<u8>,
    challenge: Vec<u8>,
    nonce: [u8; 8],
    flags: u32,
    timestamp: u64,
    created: u64,
}
impl Challenge {
    pub fn new(token: &[u8]) -> Option<(Self, Vec<u8>)> {
        let negotiate = ntlm_token(token, 0)?;
        if get32(negotiate, 8).ok()? != 1 || negotiate.len() < 32 {
            return None;
        }
        let client_flags = get32(negotiate, 12).ok()?;
        if client_flags & (UNICODE | SIGN | NTLM | EXTENDED) != UNICODE | SIGN | NTLM | EXTENDED {
            return None;
        }
        let flags = client_flags
            & (UNICODE
                | SIGN
                | NTLM
                | 0x8000
                | EXTENDED
                | VERSION
                | 0x20000000
                | KEY_EXCH
                | 0x80000000)
            | 0x00810004;
        let nonce: [u8; 8] = uuid::Uuid::new_v4().as_bytes()[..8].try_into().ok()?;
        let timestamp = (now() + 11_644_473_600) * 10_000_000;
        let target = wide("BOOTAH");
        let mut info = Vec::new();
        av(&mut info, 1, &target);
        av(&mut info, 2, &target);
        av(&mut info, 6, &2u32.to_le_bytes());
        av(&mut info, 7, &timestamp.to_le_bytes());
        av(&mut info, 0, &[]);
        let header_len = if flags & VERSION != 0 { 56 } else { 48 };
        let mut challenge = vec![0; header_len];
        challenge[..8].copy_from_slice(b"NTLMSSP\0");
        challenge[8..12].copy_from_slice(&2u32.to_le_bytes());
        write_buffer(&mut challenge, 12, target.len(), header_len);
        challenge[20..24].copy_from_slice(&flags.to_le_bytes());
        challenge[24..32].copy_from_slice(&nonce);
        write_buffer(&mut challenge, 40, info.len(), header_len + target.len());
        if flags & VERSION != 0 {
            challenge[48..56].copy_from_slice(&[6, 1, 0xb1, 0x1d, 0, 0, 0, 15]);
        }
        challenge.extend(target);
        challenge.extend(info);
        let response = response_token(Some(&challenge));
        Some((
            Self {
                negotiate: negotiate.to_vec(),
                challenge,
                nonce,
                flags,
                timestamp,
                created: now(),
            },
            response,
        ))
    }
    pub fn authenticate(
        &self,
        token: &[u8],
        credentials: &[Credential],
    ) -> Option<(Credential, [u8; 16])> {
        if now().saturating_sub(self.created) > 120 {
            return None;
        }
        let data = ntlm_token(token, 0)?;
        if get32(data, 8).ok()? != 3 || data.len() < 80 {
            return None;
        }
        let flags = get32(data, 60).ok()?;
        let security_flags = UNICODE | SIGN | NTLM | EXTENDED | KEY_EXCH | VERSION;
        if flags & security_flags != self.flags & security_flags {
            return None;
        }
        let user = utf16(security_buffer(data, 36)?).ok()?;
        let domain = utf16(security_buffer(data, 28)?).ok()?;
        let credential = credentials
            .iter()
            .find(|c| c.username.eq_ignore_ascii_case(&user) && c.expires_at > now())?;
        if !domain.is_empty() && !domain.eq_ignore_ascii_case("BOOTAH") {
            return None;
        }
        let nt = security_buffer(data, 20)?;
        if nt.len() < 48
            || nt[16..24] != [1, 1, 0, 0, 0, 0, 0, 0]
            || get64(nt, 24).ok()? != self.timestamp
        {
            return None;
        }
        let mut avs = nt.get(44..)?;
        let mut mic_required = false;
        loop {
            let id = get16(avs, 0).ok()?;
            let len = get16(avs, 2).ok()? as usize;
            let value = avs.get(4..4 + len)?;
            avs = avs.get(4 + len..)?;
            if id == 0 {
                if len != 0 {
                    return None;
                }
                break;
            }
            if id == 6 {
                mic_required = get32(value, 0).ok()? & 2 != 0;
            }
        }
        if !mic_required {
            return None;
        }
        let hash = Md4::digest(wide(&credential.password));
        let response_key = md5(&hash, &wide(&(user.to_uppercase() + &domain)));
        let mut proof_input = self.nonce.to_vec();
        proof_input.extend_from_slice(&nt[16..]);
        if !equal(&md5(&response_key, &proof_input), &nt[..16]) {
            return None;
        }
        let session_base = md5(&response_key, &nt[..16]);
        let mut session_key = session_base;
        if flags & KEY_EXCH != 0 {
            let encrypted = security_buffer(data, 52)?;
            if encrypted.len() != 16 {
                return None;
            }
            session_key.copy_from_slice(encrypted);
            let mut cipher = Rc4::<U16>::new((&session_base).into());
            cipher.apply_keystream(&mut session_key);
        } else if !security_buffer(data, 52)?.is_empty() {
            return None;
        }
        let mic_at = if flags & VERSION != 0 { 72 } else { 64 };
        let mic = data.get(mic_at..mic_at + 16)?;
        for field in [12, 20, 28, 36, 44, 52] {
            if get16(data, field).ok()? > 0 && (get32(data, field + 4).ok()? as usize) < mic_at + 16
            {
                return None;
            }
        }
        let mut input = self.negotiate.clone();
        input.extend(&self.challenge);
        let mut authenticate = data.to_vec();
        authenticate[mic_at..mic_at + 16].fill(0);
        input.extend(authenticate);
        if !equal(mic, &md5(&session_key, &input)) {
            return None;
        }
        Some((credential.clone(), session_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ntlmv2_published_response_key() {
        let hash = Md4::digest(wide("Password"));
        assert_eq!(
            md5(&hash, &wide("USERDomain")),
            [
                0x0c, 0x86, 0x8a, 0x40, 0x3b, 0xfd, 0x7a, 0x93, 0xa3, 0, 0x1e, 0xf2, 0x2e, 0xf0,
                0x2e, 0x3f
            ]
        );
    }
    #[test]
    fn signature_rejects_payload_changes() {
        let mut message = vec![0; 80];
        let expected = signature(&[42; 16], &message);
        message[79] = 1;
        assert!(!equal(&expected, &signature(&[42; 16], &message)));
        assert!(ntlm_token(&[0x60, 0x82, 0xff, 0xff], 0).is_none());
    }
}
