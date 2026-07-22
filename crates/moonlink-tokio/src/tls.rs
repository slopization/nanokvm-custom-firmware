use std::io::Cursor;

use moonlink_core::ServerIdentity;
use openssl::x509::X509;
use rustls_pemfile::{certs, pkcs8_private_keys, rsa_private_keys};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::client::danger::HandshakeSignatureValid;
use tokio_rustls::rustls::crypto::{
    WebPkiSupportedAlgorithms, aws_lc_rs, verify_tls12_signature, verify_tls13_signature,
};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use tokio_rustls::rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use tokio_rustls::rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, Error,
    ServerConfig as TlsServerConfig, SignatureScheme,
};

#[derive(Debug)]
struct MoonlightClientVerifier {
    algorithms: WebPkiSupportedAlgorithms,
}

impl MoonlightClientVerifier {
    fn new() -> Self {
        Self {
            algorithms: aws_lc_rs::default_provider().signature_verification_algorithms,
        }
    }
}

impl ClientCertVerifier for MoonlightClientVerifier {
    fn client_auth_mandatory(&self) -> bool {
        false
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        X509::from_der(end_entity.as_ref())
            .map_err(|_| Error::InvalidCertificate(CertificateError::BadEncoding))?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

pub(crate) fn create_tls_acceptor(identity: &ServerIdentity) -> Result<TlsAcceptor, String> {
    let mut cert_cursor = Cursor::new(identity.certificate_pem.as_slice());
    let certificates = certs(&mut cert_cursor)
        .collect::<Result<Vec<CertificateDer<'static>>, _>>()
        .map_err(|err| err.to_string())?;

    let mut key_cursor = Cursor::new(identity.private_key_pem.as_slice());
    let mut pkcs8_keys = pkcs8_private_keys(&mut key_cursor)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())?;
    let private_key = if let Some(key) = pkcs8_keys.pop() {
        PrivateKeyDer::Pkcs8(key)
    } else {
        let mut key_cursor = Cursor::new(identity.private_key_pem.as_slice());
        let mut rsa_keys = rsa_private_keys(&mut key_cursor)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| err.to_string())?;
        PrivateKeyDer::Pkcs1(
            rsa_keys
                .pop()
                .ok_or_else(|| "server private key is missing".to_owned())?,
        )
    };

    let config = TlsServerConfig::builder()
        .with_client_cert_verifier(std::sync::Arc::new(MoonlightClientVerifier::new()))
        .with_single_cert(certificates, private_key)
        .map_err(|err| err.to_string())?;
    Ok(TlsAcceptor::from(std::sync::Arc::new(config)))
}
