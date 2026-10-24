//! Certificate capture (RFC 0018): a rustls verifier that records the chain
//! a server sends, and the chain it verifies to, before deciding. Used by
//! every integration's HTTP client and by the handshake-only probes.

use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};

use rustls::{
    DigitallySignedStruct, RootCertStore, SignatureScheme,
    client::{
        WebPkiServerVerifier,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, TrustAnchor, UnixTime, pem::PemObject},
};
use serde::Serialize;
use x509_cert::der::Decode;

use crate::Result;

/// One certificate of a chain, as shown.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct Cert {
    /// `leaf`, `intermediate`, `root`, or `unused` (sent but not in the path)
    pub role: String,
    pub subject: String,
    pub issuer: String,
    pub not_before: i64,
    pub not_after: i64,
    /// DNS names (leaf only)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    /// a root from the bundled Mozilla set: replaced by a Homie upgrade
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub public: bool,
}

/// What a handshake showed: the chain, and why verification failed.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct Seen {
    pub chain: Vec<Cert>,
    /// `expired`, `not_yet_valid`, `unknown_issuer`, `name_mismatch` or a
    /// webpki error name
    pub problem: Option<String>,
    /// false with `tls_verify = false`: the chain is as sent, unchecked
    pub verified: bool,
}

/// Trust anchors plus the full root certificate behind each one: anchors
/// carry no validity, the roots do.
#[derive(Clone)]
pub struct Roots {
    store: Arc<RootCertStore>,
    anchors: Vec<TrustAnchor<'static>>,
    full: HashMap<(Vec<u8>, Vec<u8>), (CertificateDer<'static>, bool)>,
}

impl Roots {
    fn add(&mut self, der: CertificateDer<'static>, public: bool) {
        let Ok(a) = webpki::anchor_from_trusted_cert(&der) else {
            return;
        };
        let a = a.to_owned();
        let key = (a.subject.to_vec(), a.subject_public_key_info.to_vec());
        Arc::make_mut(&mut self.store).roots.push(a.clone());
        self.anchors.push(a);
        self.full.insert(key, (der, public));
    }
}

static PUBLIC: LazyLock<Roots> = LazyLock::new(|| {
    let mut r = Roots {
        store: Arc::new(RootCertStore::empty()),
        anchors: Vec::new(),
        full: HashMap::new(),
    };
    for c in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        r.add(c.clone().into_owned(), true);
    }
    r
});

/// The bundled roots plus the certificates in `ca_file`, if any.
pub fn roots(ca_file: Option<&Path>) -> Result<Roots> {
    let mut r = PUBLIC.clone();
    if let Some(ca) = ca_file {
        let path = crate::config::expand(ca);
        let pem = std::fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        // non-certificate sections (keys) are skipped
        for c in CertificateDer::pem_slice_iter(&pem).flatten() {
            r.add(c, false);
        }
    }
    Ok(r)
}

fn provider() -> Arc<CryptoProvider> {
    CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider()))
}

/// `CN=` of a distinguished name, else the whole name.
// ponytail: splits the RFC 4514 string on commas; an escaped comma in a CN shows truncated
fn common_name(name: &str) -> String {
    name.split(',')
        .find_map(|p| p.trim().strip_prefix("CN="))
        .unwrap_or(name)
        .to_string()
}

fn parse(der: &[u8], role: &str, public: bool) -> Option<Cert> {
    use x509_cert::ext::pkix::{SubjectAltName, name::GeneralName};
    let c = x509_cert::Certificate::from_der(der).ok()?;
    let tbs = &c.tbs_certificate;
    let secs = |t: x509_cert::time::Time| t.to_unix_duration().as_secs() as i64;
    let names = if role == "leaf" {
        match tbs.get::<SubjectAltName>() {
            Ok(Some((_, san))) => san
                .0
                .into_iter()
                .filter_map(|n| match n {
                    GeneralName::DnsName(d) => Some(d.to_string()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    // a leaf with only SANs (no CN) is named by its first DNS name
    let subject = match common_name(&tbs.subject.to_string()) {
        cn if cn.is_empty() => names.first().cloned().unwrap_or_default(),
        cn => cn,
    };
    Some(Cert {
        role: role.into(),
        subject,
        issuer: common_name(&tbs.issuer.to_string()),
        not_before: secs(tbs.validity.not_before),
        not_after: secs(tbs.validity.not_after),
        names,
        public,
    })
}

fn problem(e: &webpki::Error) -> String {
    match e {
        webpki::Error::CertExpired { .. } => "expired".into(),
        webpki::Error::CertNotValidYet { .. } => "not_yet_valid".into(),
        webpki::Error::UnknownIssuer => "unknown_issuer".into(),
        webpki::Error::CertNotValidForName(_) => "name_mismatch".into(),
        other => format!("{other:?}")
            .split(['(', ' ', '{'])
            .next()
            .unwrap_or_default()
            .to_string(),
    }
}

/// The chain the server sent and, when `verify`, the path it verifies to.
pub fn inspect(
    roots: &Roots,
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
    server_name: &ServerName<'_>,
    now: UnixTime,
    verify: bool,
) -> Seen {
    let sent = || {
        let mut chain: Vec<_> = parse(end_entity, "leaf", false).into_iter().collect();
        chain.extend(
            intermediates
                .iter()
                .filter_map(|c| parse(c, "intermediate", false)),
        );
        chain
    };
    if !verify {
        return Seen {
            chain: sent(),
            problem: None,
            verified: false,
        };
    }
    let Ok(ee) = webpki::EndEntityCert::try_from(end_entity) else {
        return Seen {
            chain: sent(),
            problem: Some("bad_certificate".into()),
            verified: true,
        };
    };
    let algs = provider().signature_verification_algorithms.all;
    let path = ee.verify_for_usage(
        algs,
        &roots.anchors,
        intermediates,
        now,
        webpki::KeyUsage::server_auth(),
        None,
        None,
    );
    let name = ee
        .verify_is_valid_for_subject_name(server_name)
        .err()
        .map(|e| problem(&e));
    match path {
        Ok(p) => {
            let mut chain: Vec<_> = parse(end_entity, "leaf", false).into_iter().collect();
            let used: Vec<_> = p.intermediate_certificates().map(|c| c.der()).collect();
            chain.extend(used.iter().filter_map(|c| parse(c, "intermediate", false)));
            let a = p.anchor();
            if let Some((der, public)) = roots
                .full
                .get(&(a.subject.to_vec(), a.subject_public_key_info.to_vec()))
            {
                chain.extend(parse(der, "root", *public));
            }
            chain.extend(
                intermediates
                    .iter()
                    .filter(|c| !used.iter().any(|u| u.as_ref() == c.as_ref()))
                    .filter_map(|c| parse(c, "unused", false)),
            );
            Seen {
                chain,
                problem: name,
                verified: true,
            }
        }
        Err(e) => Seen {
            chain: sent(),
            problem: Some(problem(&e)),
            verified: true,
        },
    }
}

/// Certificates seen by the integrations' HTTP clients, by `host:port`.
pub static ENDPOINTS: LazyLock<Mutex<BTreeMap<String, Seen>>> = LazyLock::new(Default::default);

#[derive(Debug)]
enum Sink {
    Endpoint(String),
    Probe(Arc<Mutex<Option<Seen>>>),
}

/// Records every chain, then decides: rustls' own verifier when
/// `enforce`, else accepts (`tls_verify = false`, or a probe that only
/// looks).
#[derive(Debug)]
struct Recorder {
    roots: Arc<Roots>,
    verify: bool,
    inner: Option<Arc<WebPkiServerVerifier>>,
    sink: Sink,
    provider: Arc<CryptoProvider>,
}

impl std::fmt::Debug for Roots {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Roots({})", self.anchors.len())
    }
}

impl ServerCertVerifier for Recorder {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let seen = inspect(
            &self.roots,
            end_entity,
            intermediates,
            server_name,
            now,
            self.verify,
        );
        match &self.sink {
            Sink::Endpoint(key) => {
                ENDPOINTS.lock().unwrap().insert(key.clone(), seen);
            }
            Sink::Probe(slot) => *slot.lock().unwrap() = Some(seen),
        }
        match &self.inner {
            Some(v) => v.verify_server_cert(end_entity, intermediates, server_name, ocsp, now),
            None => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn client_config(recorder: Recorder) -> Result<rustls::ClientConfig> {
    Ok(
        rustls::ClientConfig::builder_with_provider(recorder.provider.clone())
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(recorder))
            .with_no_client_auth(),
    )
}

/// The TLS config of an integration's HTTP client: verifies as before
/// (`tls_verify`, `ca_file`) and records what each handshake showed under
/// `key` (`host:port`).
pub fn http_config(key: String, roots: Roots, tls_verify: bool) -> Result<rustls::ClientConfig> {
    let provider = provider();
    let inner = tls_verify
        .then(|| {
            WebPkiServerVerifier::builder_with_provider(roots.store.clone(), provider.clone())
                .build()
        })
        .transpose()?;
    client_config(Recorder {
        roots: Arc::new(roots),
        verify: tls_verify,
        inner,
        sink: Sink::Endpoint(key),
        provider,
    })
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// A handshake with `host` (SNI and name check) at `address` (default
/// `host`), nothing sent after it. The chain is recorded even when it does
/// not verify; an error means no certificate was seen at all.
pub async fn probe(host: &str, port: u16, address: Option<&str>, roots: Roots) -> Result<Seen> {
    let slot = Arc::new(Mutex::new(None));
    let config = client_config(Recorder {
        roots: Arc::new(roots),
        verify: true,
        inner: None,
        sink: Sink::Probe(slot.clone()),
        provider: provider(),
    })?;
    let name = ServerName::try_from(host.to_string())?;
    let addr = address.unwrap_or(host);
    let tcp = tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect((addr, port)))
        .await
        .map_err(|_| format!("connecting to {addr}:{port} timed out"))??;
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    // the handshake's own result does not matter: the verifier has recorded
    let _ = tokio::time::timeout(PROBE_TIMEOUT, connector.connect(name, tcp)).await;
    slot.lock()
        .unwrap()
        .take()
        .ok_or_else(|| format!("{addr}:{port} sent no certificate").into())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, date_time_ymd,
    };

    /// A CA, an intermediate it signed, and a way to make leaves.
    pub struct Pki {
        pub root_pem: String,
        pub root: CertificateDer<'static>,
        pub inter: CertificateDer<'static>,
        inter_issuer: Issuer<'static, KeyPair>,
    }

    fn ca(cn: &str, until: i32) -> (CertificateParams, KeyPair) {
        let mut p = CertificateParams::default();
        p.distinguished_name.push(DnType::CommonName, cn);
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        p.not_before = date_time_ymd(2020, 1, 1);
        p.not_after = date_time_ymd(until, 1, 1);
        (p, KeyPair::generate().unwrap())
    }

    impl Pki {
        pub fn new(root_until: i32, inter_until: i32) -> Self {
            let (rp, rk) = ca("Test Root", root_until);
            let root = rp.self_signed(&rk).unwrap();
            let root_issuer = Issuer::new(rp, rk);
            let (ip, ik) = ca("Test Intermediate", inter_until);
            let inter = ip.signed_by(&ik, &root_issuer).unwrap();
            Self {
                root_pem: root.pem(),
                root: root.der().clone(),
                inter: inter.der().clone(),
                inter_issuer: Issuer::new(ip, ik),
            }
        }

        /// A leaf for `names`, valid between the two years, and its key.
        pub fn leaf(
            &self,
            names: &[&str],
            from: i32,
            until: i32,
        ) -> (CertificateDer<'static>, KeyPair) {
            let mut p =
                CertificateParams::new(names.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                    .unwrap();
            p.distinguished_name.push(DnType::CommonName, names[0]);
            p.not_before = date_time_ymd(from, 1, 1);
            p.not_after = date_time_ymd(until, 1, 1);
            let k = KeyPair::generate().unwrap();
            let c = p.signed_by(&k, &self.inter_issuer).unwrap();
            (c.der().clone(), k)
        }

        pub fn roots(&self) -> Roots {
            let mut r = PUBLIC.clone();
            r.add(self.root.clone(), false);
            r
        }
    }

    fn at(year: i16) -> UnixTime {
        let t: jiff::Timestamp = format!("{year}-06-01T00:00:00Z").parse().unwrap();
        UnixTime::since_unix_epoch(Duration::from_secs(t.as_second() as u64))
    }

    fn name(s: &str) -> ServerName<'static> {
        ServerName::try_from(s.to_string()).unwrap()
    }

    #[test]
    fn verified_path_includes_the_private_root() {
        let pki = Pki::new(2040, 2035);
        let (leaf, _) = pki.leaf(&["a.example"], 2024, 2030);
        let s = inspect(
            &pki.roots(),
            &leaf,
            std::slice::from_ref(&pki.inter),
            &name("a.example"),
            at(2026),
            true,
        );
        assert_eq!(s.problem, None);
        let roles: Vec<_> = s
            .chain
            .iter()
            .map(|c| (c.role.as_str(), c.subject.as_str(), c.public))
            .collect();
        assert_eq!(
            roles,
            [
                ("leaf", "a.example", false),
                ("intermediate", "Test Intermediate", false),
                ("root", "Test Root", false)
            ]
        );
        assert_eq!(s.chain[0].names, ["a.example"]);
        assert_eq!(s.chain[0].issuer, "Test Intermediate");
    }

    #[test]
    fn problems_keep_the_sent_chain() {
        let pki = Pki::new(2040, 2035);
        let (leaf, _) = pki.leaf(&["a.example"], 2024, 2025);
        let roots = pki.roots();
        let inter = [pki.inter.clone()];
        let s = inspect(&roots, &leaf, &inter, &name("a.example"), at(2026), true);
        assert_eq!(s.problem.as_deref(), Some("expired"));
        assert_eq!(s.chain.len(), 2);
        let s = inspect(&roots, &leaf, &inter, &name("a.example"), at(2023), true);
        assert_eq!(s.problem.as_deref(), Some("not_yet_valid"));
        // the intermediate was not sent
        let s = inspect(&roots, &leaf, &[], &name("a.example"), at(2024), true);
        assert_eq!(s.problem.as_deref(), Some("unknown_issuer"));
        assert_eq!(s.chain.len(), 1);
        let s = inspect(&roots, &leaf, &inter, &name("b.example"), at(2024), true);
        assert_eq!(s.problem.as_deref(), Some("name_mismatch"));
        // an untrusted root
        let s = inspect(
            &PUBLIC.clone(),
            &leaf,
            &inter,
            &name("a.example"),
            at(2024),
            true,
        );
        assert_eq!(s.problem.as_deref(), Some("unknown_issuer"));
        // verification off: as sent, no problem
        let s = inspect(
            &PUBLIC.clone(),
            &leaf,
            &inter,
            &name("a.example"),
            at(2026),
            false,
        );
        assert_eq!((s.problem, s.verified, s.chain.len()), (None, false, 2));
    }

    #[test]
    fn unused_intermediates_are_listed() {
        let pki = Pki::new(2040, 2035);
        let other = Pki::new(2040, 2035);
        let (leaf, _) = pki.leaf(&["a.example"], 2024, 2030);
        let s = inspect(
            &pki.roots(),
            &leaf,
            &[pki.inter.clone(), other.inter.clone()],
            &name("a.example"),
            at(2026),
            true,
        );
        assert_eq!(s.problem, None);
        assert_eq!(s.chain.last().unwrap().role, "unused");
    }

    #[test]
    fn bundled_roots_are_public() {
        assert!(PUBLIC.full.values().all(|(_, public)| *public));
        assert!(PUBLIC.anchors.len() > 100);
        let r = roots(Some(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/test-rsa.pem"
        ))))
        .unwrap();
        assert_eq!(r.anchors.len(), PUBLIC.anchors.len());
        assert!(roots(Some(Path::new("/nonexistent/ca.pem"))).is_err());
    }

    #[test]
    fn a_leaf_without_cn_is_named_by_its_san() {
        let pki = Pki::new(2040, 2035);
        let mut p = CertificateParams::new(vec!["san-only.example".to_string()]).unwrap();
        p.distinguished_name = rcgen::DistinguishedName::new();
        let k = KeyPair::generate().unwrap();
        let leaf = p.signed_by(&k, &pki.inter_issuer).unwrap();
        let c = parse(leaf.der(), "leaf", false).unwrap();
        assert_eq!(c.subject, "san-only.example");
    }

    #[test]
    fn common_names() {
        assert_eq!(common_name("CN=R11,O=Let's Encrypt,C=US"), "R11");
        assert_eq!(common_name("O=Example"), "O=Example");
    }

    /// A TLS server on a loopback port serving `chain`.
    pub async fn tls_server(chain: Vec<CertificateDer<'static>>, key: &KeyPair) -> u16 {
        let key = rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der()).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((s, _)) = listener.accept().await {
                let a = acceptor.clone();
                tokio::spawn(async move {
                    let _ = a.accept(s).await;
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn probe_records_even_failed_handshakes() {
        let pki = Pki::new(2040, 2035);
        let (leaf, key) = pki.leaf(&["a.example"], 2020, 2021);
        let port = tls_server(vec![leaf, pki.inter.clone()], &key).await;
        let s = probe("a.example", port, Some("127.0.0.1"), pki.roots())
            .await
            .unwrap();
        assert_eq!(s.problem.as_deref(), Some("expired"));
        assert_eq!(s.chain[0].subject, "a.example");
        // nothing listens on port 1
        assert!(
            probe("a.example", 1, Some("127.0.0.1"), pki.roots())
                .await
                .is_err()
        );
    }
}
