use moonlink_core::{
    ClientIdentity, PairingStore, ServerError, ServerIdentity, TrustedClient,
    client_id_from_certificate,
};
use openssl::asn1::Asn1Time;
use openssl::bn::{BigNum, MsbOption};
use openssl::hash::{MessageDigest, hash};
use openssl::nid::Nid;
use openssl::pkey::{PKey, Private};
use openssl::rsa::Rsa;
use openssl::sign::{Signer, Verifier};
use openssl::symm::{Cipher, Crypter, Mode};
use openssl::x509::{X509, X509NameBuilder};
use uuid::Uuid;

use crate::shared::PairingSessions;
use crate::xml::read_query_value;

#[derive(Debug, Clone)]
pub(crate) struct PairingSession {
    pub(crate) key: Vec<u8>,
    pub(crate) server_secret: Vec<u8>,
    pub(crate) server_challenge: Vec<u8>,
    pub(crate) client_hash: Vec<u8>,
    pub(crate) client_certificate_der: Option<Vec<u8>>,
}

pub(crate) async fn ensure_server_identity<S>(store: &S) -> Result<ServerIdentity, ServerError>
where
    S: PairingStore,
{
    if let Some(identity) = store.server_identity().await? {
        return Ok(identity);
    }
    let identity = generate_self_signed_identity()?;
    store.put_server_identity(identity.clone()).await?;
    Ok(identity)
}

pub(crate) async fn handle_pair<S>(
    path: &str,
    peer: &str,
    pin: &str,
    store: &S,
    sessions: PairingSessions,
) -> Result<String, ServerError>
where
    S: PairingStore,
{
    let identity = ensure_server_identity(store).await?;
    let unique_id =
        read_query_value(path, "uniqueid").unwrap_or_else(|| Uuid::new_v4().to_string());
    let session_id = format!("{peer}/{unique_id}");

    if read_query_value(path, "phrase").as_deref() == Some("pairchallenge") {
        return Ok(build_xml_status(&[("paired", "1".to_owned())]));
    }

    if read_query_value(path, "phrase").as_deref() == Some("getservercert") {
        let salt = read_query_value(path, "salt")
            .and_then(|value| decode_hex(&value))
            .unwrap_or_default();
        let client_certificate_der = read_query_value(path, "clientcert")
            .and_then(|value| decode_hex(&value))
            .unwrap_or_default();
        let key = derive_pairing_aes_key(&salt, pin)?;
        sessions.lock().await.insert(
            session_id,
            PairingSession {
                key,
                server_secret: Vec::new(),
                server_challenge: Vec::new(),
                client_hash: Vec::new(),
                client_certificate_der: Some(client_certificate_der),
            },
        );
        return Ok(build_xml_status(&[
            ("paired", "1".to_owned()),
            ("plaincert", encode_hex(&identity.certificate_pem)),
        ]));
    }

    if let Some(client_cert) = read_query_value(path, "clientcert") {
        let cert = decode_hex(&client_cert).unwrap_or_default();
        let mut sessions = sessions.lock().await;
        let session = sessions
            .entry(session_id.clone())
            .or_insert_with(|| PairingSession {
                key: Vec::new(),
                server_secret: generate_random_bytes(16).unwrap_or_default(),
                server_challenge: Vec::new(),
                client_hash: Vec::new(),
                client_certificate_der: None,
            });
        session.client_certificate_der = Some(cert);
        return Ok(build_xml_status(&[
            ("paired", "1".to_owned()),
            ("servercert", encode_hex(&identity.certificate_pem)),
        ]));
    }

    if let Some(client_challenge) = read_query_value(path, "clientchallenge") {
        let challenge = decode_hex(&client_challenge).unwrap_or_default();
        let mut sessions = sessions.lock().await;
        let Some(session) = sessions.get_mut(&session_id) else {
            return Ok(build_xml_status(&[("paired", "0".to_owned())]));
        };
        let plain = decrypt_aes_128_ecb(&session.key, &challenge).unwrap_or_default();
        let server_signature = read_cert_signature(&identity.certificate_pem)?;
        let server_secret = generate_random_bytes(16)?;
        let mut hash_input =
            Vec::with_capacity(plain.len() + server_signature.len() + server_secret.len());
        hash_input.extend_from_slice(&plain);
        hash_input.extend_from_slice(&server_signature);
        hash_input.extend_from_slice(&server_secret);
        let hash = hash_sha256(&hash_input)?;
        let server_challenge = generate_random_bytes(16)?;
        let mut response = Vec::with_capacity(hash.len() + server_challenge.len());
        response.extend_from_slice(&hash);
        response.extend_from_slice(&server_challenge);
        let encrypted = encrypt_aes_128_ecb(&session.key, &response)?;
        session.server_secret = server_secret;
        session.server_challenge = server_challenge;
        return Ok(build_xml_status(&[
            ("paired", "1".to_owned()),
            ("challengeresponse", encode_hex(&encrypted)),
        ]));
    }

    if let Some(server_challenge_response) = read_query_value(path, "serverchallengeresp") {
        let encrypted_response = decode_hex(&server_challenge_response).unwrap_or_default();
        let mut sessions = sessions.lock().await;
        let Some(session) = sessions.get_mut(&session_id) else {
            return Ok(build_xml_status(&[("paired", "0".to_owned())]));
        };
        session.client_hash =
            decrypt_aes_128_ecb(&session.key, &encrypted_response).unwrap_or_default();
        let private_key = PKey::private_key_from_pem(&identity.private_key_pem)
            .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
        let signature = sign_sha256(&private_key, &session.server_secret)?;
        let mut secret = session.server_secret.clone();
        secret.extend_from_slice(&signature);
        return Ok(build_xml_status(&[
            ("paired", "1".to_owned()),
            ("pairingsecret", encode_hex(&secret)),
        ]));
    }

    if let Some(client_pairing_secret) = read_query_value(path, "clientpairingsecret") {
        let client_pairing_secret = decode_hex(&client_pairing_secret).unwrap_or_default();
        if let Some(certificate_der) =
            sessions
                .lock()
                .await
                .remove(&session_id)
                .and_then(|session| {
                    verify_client_pairing_secret(&session, &client_pairing_secret)
                        .then_some(session.client_certificate_der)
                        .flatten()
                })
        {
            let certificate_der = parse_certificate(&certificate_der)?
                .to_der()
                .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
            store
                .trust_client(TrustedClient {
                    identity: ClientIdentity {
                        id: client_id_from_certificate(&certificate_der),
                        certificate_der,
                    },
                    display_name: read_query_value(path, "devicename"),
                })
                .await?;
            return Ok(build_xml_status(&[("paired", "1".to_owned())]));
        }
        return Ok(build_xml_status(&[("paired", "0".to_owned())]));
    }

    Ok(build_xml_status(&[("paired", "0".to_owned())]))
}

fn generate_self_signed_identity() -> Result<ServerIdentity, ServerError> {
    let rsa = Rsa::generate(2048).map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    let private_key =
        PKey::from_rsa(rsa).map_err(|err| ServerError::InvalidConfig(err.to_string()))?;

    let mut name =
        X509NameBuilder::new().map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    name.append_entry_by_nid(Nid::COMMONNAME, "Moonlink")
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    let name = name.build();

    let mut serial = BigNum::new().map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    serial
        .rand(128, MsbOption::MAYBE_ZERO, false)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    let serial = serial
        .to_asn1_integer()
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;

    let mut cert = X509::builder().map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    cert.set_version(2)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    cert.set_serial_number(&serial)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    cert.set_subject_name(&name)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    cert.set_issuer_name(&name)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    cert.set_pubkey(&private_key)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    let not_before =
        Asn1Time::days_from_now(0).map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    let not_after =
        Asn1Time::days_from_now(3650).map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    cert.set_not_before(&not_before)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    cert.set_not_after(&not_after)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    cert.sign(&private_key, MessageDigest::sha256())
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;

    Ok(ServerIdentity {
        id: Uuid::new_v4(),
        certificate_pem: cert
            .build()
            .to_pem()
            .map_err(|err| ServerError::InvalidConfig(err.to_string()))?,
        private_key_pem: private_key
            .private_key_to_pem_pkcs8()
            .map_err(|err| ServerError::InvalidConfig(err.to_string()))?,
    })
}

fn derive_pairing_aes_key(salt: &[u8], pin: &str) -> Result<Vec<u8>, ServerError> {
    let mut input = Vec::with_capacity(salt.len() + pin.len());
    input.extend_from_slice(salt);
    input.extend_from_slice(pin.as_bytes());
    Ok(hash(MessageDigest::sha256(), &input)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?[..16]
        .to_vec())
}

fn hash_sha256(data: &[u8]) -> Result<Vec<u8>, ServerError> {
    Ok(hash(MessageDigest::sha256(), data)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?
        .to_vec())
}

fn encrypt_aes_128_ecb(key: &[u8], data: &[u8]) -> Result<Vec<u8>, ServerError> {
    process_aes_128_ecb(Mode::Encrypt, key, data)
}

fn decrypt_aes_128_ecb(key: &[u8], data: &[u8]) -> Result<Vec<u8>, ServerError> {
    process_aes_128_ecb(Mode::Decrypt, key, data)
}

fn process_aes_128_ecb(mode: Mode, key: &[u8], data: &[u8]) -> Result<Vec<u8>, ServerError> {
    let mut crypter = Crypter::new(Cipher::aes_128_ecb(), mode, key, None)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    // GameStream pairing uses raw AES-ECB blocks without PKCS padding.
    crypter.pad(false);
    let mut out = vec![0; data.len() + Cipher::aes_128_ecb().block_size()];
    let mut count = crypter
        .update(data, &mut out)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    count += crypter
        .finalize(&mut out[count..])
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    out.truncate(count);
    Ok(out)
}

#[allow(dead_code)]
fn read_cert_signature(certificate_der: &[u8]) -> Result<Vec<u8>, ServerError> {
    let certificate = parse_certificate(certificate_der)?;
    Ok(certificate.signature().as_slice().to_vec())
}

#[allow(dead_code)]
fn sign_sha256(key: &PKey<Private>, data: &[u8]) -> Result<Vec<u8>, ServerError> {
    let mut signer = Signer::new(MessageDigest::sha256(), key)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    signer
        .update(data)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    signer
        .sign_to_vec()
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))
}

#[allow(dead_code)]
fn verify_sha256(
    certificate_der: &[u8],
    data: &[u8],
    signature: &[u8],
) -> Result<bool, ServerError> {
    let certificate = parse_certificate(certificate_der)?;
    let key = certificate
        .public_key()
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    let mut verifier = Verifier::new(MessageDigest::sha256(), &key)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    verifier
        .update(data)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    verifier
        .verify(signature)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))
}

fn parse_certificate(certificate: &[u8]) -> Result<X509, ServerError> {
    X509::from_pem(certificate)
        .or_else(|_| X509::from_der(certificate))
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))
}

fn verify_client_pairing_secret(session: &PairingSession, secret: &[u8]) -> bool {
    let Some(client_certificate) = &session.client_certificate_der else {
        return false;
    };
    if secret.len() <= 16 {
        return false;
    }
    let (client_secret, signature) = secret.split_at(16);
    let Ok(client_cert_signature) = read_cert_signature(client_certificate) else {
        return false;
    };
    let mut hash_input = Vec::with_capacity(
        session.server_challenge.len() + client_cert_signature.len() + client_secret.len(),
    );
    hash_input.extend_from_slice(&session.server_challenge);
    hash_input.extend_from_slice(&client_cert_signature);
    hash_input.extend_from_slice(client_secret);
    let Ok(client_hash) = hash_sha256(&hash_input) else {
        return false;
    };
    client_hash == session.client_hash
        && verify_sha256(client_certificate, client_secret, signature).unwrap_or(false)
}

fn generate_random_bytes(len: usize) -> Result<Vec<u8>, ServerError> {
    let mut bytes = vec![0_u8; len];
    openssl::rand::rand_bytes(&mut bytes)
        .map_err(|err| ServerError::InvalidConfig(err.to_string()))?;
    Ok(bytes)
}

fn build_xml_status(fields: &[(&str, String)]) -> String {
    let mut xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\">".to_owned();
    for (name, value) in fields {
        xml.push('<');
        xml.push_str(name);
        xml.push('>');
        xml.push_str(value);
        xml.push_str("</");
        xml.push_str(name);
        xml.push('>');
    }
    xml.push_str("</root>");
    xml
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).ok())
        .collect()
}
