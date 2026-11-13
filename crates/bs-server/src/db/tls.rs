//! TLS к PostgreSQL с режимами, как у libpq: `sslmode` и `sslrootcert` в
//! адресе базы.
//!
//! - `disable` — без шифрования;
//! - `prefer` (по умолчанию, как у libpq) — шифрование, если сервер его
//!   предлагает, без проверки сертификата;
//! - `require` — только с шифрованием; сертификат проверяется, лишь когда
//!   задан `sslrootcert`;
//! - `verify-ca` — шифрование и цепочка до доверенного центра;
//! - `verify-full` — то же и имя узла в сертификате.
//!
//! `sslrootcert` — файл PEM с сертификатами центров или `system` (системные
//! корни и корни Mozilla); без него `verify-*` доверяют системным корням.

use std::sync::Arc;

use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, Error, RootCertStore, SignatureScheme,
};

/// Режим TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Без шифрования.
    Disable,
    /// Шифрование, если сервер предлагает.
    Prefer,
    /// Только с шифрованием.
    Require,
    /// Шифрование и проверка цепочки.
    VerifyCa,
    /// Шифрование, цепочка и имя узла.
    VerifyFull,
}

/// Адрес базы без параметров TLS и сами параметры.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Адрес для драйвера (без `sslmode` и `sslrootcert`).
    pub url: String,
    /// Режим.
    pub mode: Mode,
    /// `sslrootcert`.
    pub root: Option<String>,
}

fn mode_of(value: &str) -> Result<Mode, String> {
    Ok(match value {
        "disable" => Mode::Disable,
        "prefer" | "allow" => Mode::Prefer,
        "require" => Mode::Require,
        "verify-ca" => Mode::VerifyCa,
        "verify-full" => Mode::VerifyFull,
        other => return Err(format!("неизвестный sslmode: {other}")),
    })
}

/// Разбирает адрес базы: URL (`postgres://…?sslmode=…`) или строку
/// `ключ=значение`.
///
/// # Errors
/// Адрес не разобрать или неизвестный `sslmode`.
pub fn parse(address: &str) -> Result<Target, String> {
    let mut mode = Mode::Prefer;
    let mut root = None;
    if address.starts_with("postgres://") || address.starts_with("postgresql://") {
        let mut url = url::Url::parse(address).map_err(|error| format!("адрес базы: {error}"))?;
        let mut kept = Vec::new();
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "sslmode" => mode = mode_of(&value)?,
                "sslrootcert" => root = Some(value.into_owned()),
                _ => kept.push((key.into_owned(), value.into_owned())),
            }
        }
        if kept.is_empty() {
            url.set_query(None);
        } else {
            url.query_pairs_mut().clear().extend_pairs(kept);
        }
        return Ok(Target {
            url: url.into(),
            mode,
            root,
        });
    }
    let mut kept = Vec::new();
    for part in address.split_whitespace() {
        match part.split_once('=') {
            Some(("sslmode", value)) => mode = mode_of(value)?,
            Some(("sslrootcert", value)) => root = Some(value.to_owned()),
            _ => kept.push(part),
        }
    }
    Ok(Target {
        url: kept.join(" "),
        mode,
        root,
    })
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Доверенные центры: файл `sslrootcert` или системные корни.
///
/// # Errors
/// Файл не прочитать или в нём нет сертификатов.
fn roots(root: Option<&str>) -> Result<RootCertStore, String> {
    let mut store = RootCertStore::empty();
    match root {
        Some(path) if path != "system" => {
            for cert in CertificateDer::pem_file_iter(path)
                .map_err(|error| format!("sslrootcert {path}: {error}"))?
            {
                let cert = cert.map_err(|error| format!("sslrootcert {path}: {error}"))?;
                store
                    .add(cert)
                    .map_err(|error| format!("sslrootcert {path}: {error}"))?;
            }
            if store.is_empty() {
                return Err(format!("sslrootcert {path}: нет сертификатов"));
            }
        }
        _ => {
            store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let native = rustls_native_certs::load_native_certs();
            for cert in native.certs {
                let _ = store.add(cert);
            }
        }
    }
    Ok(store)
}

/// Проверка, которая принимает любой сертификат (шифрование без проверки).
#[derive(Debug)]
struct Unverified(WebPkiSupportedAlgorithms);

impl ServerCertVerifier for Unverified {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_schemes()
    }
}

/// Проверка цепочки без имени узла (`verify-ca`).
#[derive(Debug)]
struct ChainOnly(Arc<WebPkiServerVerifier>);

impl ServerCertVerifier for ChainOnly {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        match self
            .0
            .verify_server_cert(end_entity, intermediates, server_name, ocsp, now)
        {
            Err(Error::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) => Ok(ServerCertVerified::assertion()),
            other => other,
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.0.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.0.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.supported_verify_schemes()
    }
}

/// Настройки клиента TLS для режима (для `disable` не нужны).
///
/// # Errors
/// Корни не прочитать.
pub fn client_config(target: &Target) -> Result<ClientConfig, String> {
    let provider = provider();
    let builder = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|error| error.to_string())?;
    let verify = match target.mode {
        Mode::VerifyCa | Mode::VerifyFull => true,
        Mode::Require => target.root.is_some(),
        Mode::Disable | Mode::Prefer => false,
    };
    let config = if verify {
        let store = Arc::new(roots(target.root.as_deref())?);
        let webpki = WebPkiServerVerifier::builder_with_provider(store, provider)
            .build()
            .map_err(|error| error.to_string())?;
        if target.mode == Mode::VerifyFull {
            builder.with_webpki_verifier(webpki).with_no_client_auth()
        } else {
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(ChainOnly(webpki)))
                .with_no_client_auth()
        }
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Unverified(
                provider.signature_verification_algorithms,
            )))
            .with_no_client_auth()
    };
    Ok(config)
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "тесты")]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let t = parse(
            "postgres://u@h:5432/db?sslmode=verify-full&sslrootcert=/ca.pem&application_name=x",
        )
        .expect("адрес");
        assert_eq!(t.mode, Mode::VerifyFull);
        assert_eq!(t.root.as_deref(), Some("/ca.pem"));
        assert_eq!(t.url, "postgres://u@h:5432/db?application_name=x");
        let t = parse("postgres://u@h/db").expect("адрес");
        assert_eq!(
            (t.mode, t.url.as_str()),
            (Mode::Prefer, "postgres://u@h/db")
        );
        let t = parse("postgresql://u@h/db?sslmode=disable").expect("адрес");
        assert_eq!(
            (t.mode, t.url.as_str()),
            (Mode::Disable, "postgresql://u@h/db")
        );
        assert!(parse("postgres://u@h/db?sslmode=strict").is_err());
    }

    #[test]
    fn key_value() {
        let t = parse("host=h user=u sslmode=require dbname=d").expect("адрес");
        assert_eq!(t.mode, Mode::Require);
        assert_eq!(t.url, "host=h user=u dbname=d");
    }

    #[test]
    fn configs() {
        for mode in [
            Mode::Prefer,
            Mode::Require,
            Mode::VerifyCa,
            Mode::VerifyFull,
        ] {
            let target = Target {
                url: String::new(),
                mode,
                root: None,
            };
            assert!(client_config(&target).is_ok());
        }
        let missing = Target {
            url: String::new(),
            mode: Mode::VerifyFull,
            root: Some("/нет/такого.pem".to_owned()),
        };
        assert!(client_config(&missing).is_err());
    }
}
