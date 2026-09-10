use std::{fmt, net::SocketAddr, str::FromStr};

use serde::{Deserialize, Serialize};
use whisperia_proto::{control::Password, DEFAULT_PORT};

use crate::{discovery::DiscoveredServer, trust::Fingerprint};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ServerAddress {
    host: String,
    port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("an address looks like 192.168.1.52 or host-pc:47820")]
pub struct NotAnAddress;

impl ServerAddress {
    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl From<SocketAddr> for ServerAddress {
    fn from(addr: SocketAddr) -> Self {
        Self {
            host: addr.ip().to_string(),
            port: addr.port(),
        }
    }
}

impl FromStr for ServerAddress {
    type Err = NotAnAddress;

    fn from_str(input: &str) -> Result<Self, NotAnAddress> {
        let input = input.trim();
        if input.is_empty() || input.contains(char::is_whitespace) {
            return Err(NotAnAddress);
        }
        let (host, port) = match input.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() && !host.contains(':') => {
                let port = port.parse().ok().filter(|p| *p != 0).ok_or(NotAnAddress)?;
                (host, port)
            }
            Some(_) => return Err(NotAnAddress),
            None => (input, DEFAULT_PORT),
        };
        Ok(Self {
            host: host.to_owned(),
            port,
        })
    }
}

impl TryFrom<String> for ServerAddress {
    type Error = NotAnAddress;

    fn try_from(text: String) -> Result<Self, NotAnAddress> {
        text.parse()
    }
}

impl From<ServerAddress> for String {
    fn from(address: ServerAddress) -> String {
        address.to_string()
    }
}

impl fmt::Display for ServerAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.port == DEFAULT_PORT {
            f.write_str(&self.host)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Discovered(Vec<SocketAddr>),
    Typed(ServerAddress),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub label: String,
    pub trust_key: String,
    pub expected: Option<Fingerprint>,
    pub route: Route,
    pub password: Option<Password>,
}

impl Target {
    pub fn discovered(server: &DiscoveredServer) -> Self {
        Self {
            label: server.name.to_string(),
            trust_key: server.name.to_string(),
            expected: server.id,
            route: Route::Discovered(server.addrs.clone()),
            password: None,
        }
    }

    pub fn typed(address: ServerAddress, expected: Option<Fingerprint>) -> Self {
        Self {
            label: address.to_string(),
            trust_key: address.to_string(),
            expected,
            route: Route::Typed(address),
            password: None,
        }
    }

    pub fn typed_address(&self) -> Option<&ServerAddress> {
        match &self.route {
            Route::Typed(address) => Some(address),
            Route::Discovered(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(text: &str) -> Result<ServerAddress, NotAnAddress> {
        text.parse()
    }

    #[test]
    fn an_address_without_a_port_uses_the_usual_one() {
        let home = address(" 192.168.1.5 ").unwrap();
        assert_eq!((home.host(), home.port()), ("192.168.1.5", DEFAULT_PORT));
        assert_eq!(home.to_string(), "192.168.1.5");
    }

    #[test]
    fn an_address_keeps_another_port() {
        let other = address("desktop-av83:5000").unwrap();
        assert_eq!((other.host(), other.port()), ("desktop-av83", 5000));
        assert_eq!(other.to_string(), "desktop-av83:5000");
    }

    #[test]
    fn written_differently_is_the_same_address() {
        assert_eq!(address("192.168.1.5"), address("192.168.1.5:47820"));
    }

    #[test]
    fn what_is_not_an_address_is_refused() {
        for bad in [
            "desktop-av83:",
            "desktop-av83:0",
            "desktop-av83:65536",
            "desktop av83",
            "::1",
            "[::1]:5000",
            "",
        ] {
            assert_eq!(address(bad), Err(NotAnAddress), "{bad}");
        }
    }

    #[test]
    fn an_address_is_kept_as_it_is_shown() {
        let json = serde_json::to_string(&address("pc:5000").unwrap()).unwrap();
        assert_eq!(json, "\"pc:5000\"");
        assert!(serde_json::from_str::<ServerAddress>("\"a b\"").is_err());
    }
}
