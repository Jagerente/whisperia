use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};

use tokio::runtime::Runtime;
use whisperia_proto::control::{Name, Password};
use whisperia_server::{Server, ServerConfig, ServerError};

use crate::TempDir;

const REBIND_PATIENCE: Duration = Duration::from_secs(5);

pub struct TestServerBuilder {
    name: String,
    password: Option<String>,
    max_peers: usize,
    advertise: bool,
    everywhere: bool,
    port: u16,
}

impl TestServerBuilder {
    pub fn password(mut self, password: &str) -> Self {
        self.password = Some(password.into());
        self
    }

    pub fn max_peers(mut self, max_peers: usize) -> Self {
        self.max_peers = max_peers;
        self
    }

    pub fn advertised(mut self) -> Self {
        self.advertise = true;
        self.everywhere = true;
        self
    }

    pub fn on_every_address(mut self) -> Self {
        self.everywhere = true;
        self
    }

    pub fn on_port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    pub fn start(self) -> TestServer {
        let dir = TempDir::new("server");
        let listen_on = if self.everywhere {
            Ipv4Addr::UNSPECIFIED
        } else {
            Ipv4Addr::LOCALHOST
        };
        let config = ServerConfig {
            name: Name::new(&self.name).expect("a test server has a name"),
            bind: SocketAddr::new(IpAddr::V4(listen_on), self.port),
            password: self
                .password
                .as_deref()
                .and_then(|typed| Password::new(typed).ok()),
            data_dir: dir.path().to_owned(),
            max_peers: self.max_peers,
            advertise: self.advertise,
            advertise_addrs: Vec::new(),
        };
        let mut server = TestServer {
            server: None,
            config,
            runtime: Some(Runtime::new().expect("a runtime for the server")),
            _dir: dir,
        };
        server.start_again();
        server
    }
}

pub struct TestServer {
    server: Option<Server>,
    config: ServerConfig,
    runtime: Option<Runtime>,
    _dir: TempDir,
}

impl TestServer {
    pub fn named(name: &str) -> TestServerBuilder {
        TestServerBuilder {
            name: name.into(),
            password: None,
            max_peers: 8,
            advertise: false,
            everywhere: false,
            port: 0,
        }
    }

    pub fn start(name: &str) -> Self {
        Self::named(name).start()
    }

    fn running(&self) -> &Server {
        self.server.as_ref().expect("the server is running")
    }

    fn runtime(&self) -> &Runtime {
        self.runtime
            .as_ref()
            .expect("the server's runtime is there")
    }

    pub fn name(&self) -> &str {
        &self.config.name
    }

    pub fn port(&self) -> u16 {
        self.config.bind.port()
    }

    pub fn addr(&self) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.port())
    }

    pub fn address(&self) -> String {
        self.addr().to_string()
    }

    pub fn fingerprint(&self) -> [u8; 32] {
        self.running().fingerprint()
    }

    pub fn occupancy(&self) -> (usize, usize) {
        self.running().occupancy()
    }

    pub async fn shutdown(&mut self) {
        if let Some(server) = self.server.take() {
            server.shutdown().await;
        }
    }

    pub fn stop(&mut self) {
        if let Some(server) = self.server.take() {
            self.runtime().block_on(server.shutdown());
        }
    }

    pub fn start_again(&mut self) {
        let runtime = self
            .runtime
            .as_ref()
            .expect("the server's runtime is there");
        let _entered = runtime.enter();
        let server = listen(&self.config)
            .unwrap_or_else(|e| panic!("the server cannot listen on {}: {e}", self.config.bind));
        if let Some(port) = server.local_addr().map(|addr| addr.port()) {
            self.config.bind.set_port(port);
        }
        self.server = Some(server);
    }

    pub fn restart(&mut self) {
        self.stop();
        self.start_again();
    }
}

fn listen(config: &ServerConfig) -> Result<Server, ServerError> {
    let deadline = Instant::now() + REBIND_PATIENCE;
    loop {
        match Server::start(config.clone()) {
            Err(ServerError::Bind { .. })
                if config.bind.port() != 0 && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            outcome => return outcome,
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        drop(self.server.take());
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}
