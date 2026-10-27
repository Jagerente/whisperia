use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use mdns_sd::{ServiceDaemon, ServiceInfo};
use rustls::pki_types::PrivatePkcs8KeyDer;
use whisperia_proto::{
    control::encode_frame, peek::Glance, ALPN, ALPN_PEEK, MDNS_SERVICE_TYPE, MDNS_TXT_NAME,
    MDNS_TXT_PEERS, MDNS_TXT_PROTOCOL, MDNS_TXT_ROOMS, PROTOCOL_VERSION, TLS_SERVER_NAME,
};

pub fn quic_server(alpn: &[u8]) -> (quinn::Endpoint, SocketAddr) {
    let made = rcgen::generate_simple_self_signed(vec![TLS_SERVER_NAME.to_owned()]).unwrap();
    let key = PrivatePkcs8KeyDer::from(made.signing_key.serialize_der());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![made.cert.der().clone()], key.into())
        .unwrap();
    tls.alpn_protocols = vec![alpn.to_vec()];
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap();
    let config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    let endpoint = quinn::Endpoint::server(config, "0.0.0.0:0".parse().unwrap()).unwrap();
    let port = endpoint.local_addr().unwrap().port();
    (
        endpoint,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
    )
}

pub fn server_from_before_peeks() -> (quinn::Endpoint, SocketAddr) {
    let (endpoint, addr) = quic_server(ALPN);
    let accepting = endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = accepting.accept().await {
            let _ = incoming.await;
        }
    });
    (endpoint, addr)
}

pub fn server_that_hangs_up_when_asked() -> (quinn::Endpoint, SocketAddr) {
    let (endpoint, addr) = quic_server(ALPN_PEEK);
    let accepting = endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = accepting.accept().await {
            if let Ok(conn) = incoming.await {
                conn.close(0u32.into(), b"server shutting down");
            }
        }
    });
    (endpoint, addr)
}

pub fn slow_server(name: &str, delay: Duration) -> (quinn::Endpoint, SocketAddr) {
    let (endpoint, addr) = quic_server(ALPN_PEEK);
    let accepting = endpoint.clone();
    let name = name.to_owned();
    tokio::spawn(async move {
        while let Some(incoming) = accepting.accept().await {
            let name = name.clone();
            tokio::spawn(async move {
                let Ok(conn) = incoming.await else {
                    return;
                };
                tokio::time::sleep(delay).await;
                let glance = Glance {
                    protocol: PROTOCOL_VERSION,
                    name,
                    rooms: 1,
                    people: 2,
                };
                if let Ok(mut send) = conn.open_uni().await {
                    let _ = send.write_all(&encode_frame(&glance).unwrap()).await;
                    let _ = send.finish();
                }
                conn.closed().await;
            });
        }
    });
    (endpoint, addr)
}

pub fn announce(name: &str, protocol: u16, port: u16) -> ServiceDaemon {
    let daemon = ServiceDaemon::new().unwrap();
    let id = format!("{}-{}", name.to_lowercase(), std::process::id());
    let properties = [
        (MDNS_TXT_NAME, name.to_owned()),
        (MDNS_TXT_PROTOCOL, protocol.to_string()),
        (MDNS_TXT_ROOMS, "0".to_owned()),
        (MDNS_TXT_PEERS, "0".to_owned()),
    ];
    let service = ServiceInfo::new(
        MDNS_SERVICE_TYPE,
        &id,
        &format!("ww-{id}.local."),
        "",
        port,
        &properties[..],
    )
    .unwrap()
    .enable_addr_auto();
    daemon.register(service).unwrap();
    daemon
}
