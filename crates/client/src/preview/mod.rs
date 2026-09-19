#[cfg(any(test, feature = "testing"))]
pub mod fake;
mod page;
mod picture;

use std::{
    io::Read,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs},
    time::Duration,
};

use self::page::Page;
pub use self::picture::{Picture, PictureError};
use crate::app::message::Url;

const MOST_PAGE_BYTES: u64 = 512 * 1024;
const MOST_PICTURE_BYTES: u64 = 8 * 1024 * 1024;
const MOST_REDIRECTS: usize = 4;
const TIMEOUT: Duration = Duration::from_secs(8);
const PAGE_KINDS: &str = "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1";
const PICTURE_KINDS: &str = "image/avif,image/webp,image/png,image/jpeg,image/gif,*/*;q=0.1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPreview {
    pub site: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub picture: Option<Picture>,
    pub colour: Option<u32>,
}

#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error("{0} is not a web address")]
    NotWeb(String),
    #[error("{0} is not on the internet")]
    NotPublic(String),
    #[error("cannot find {host}: {source}")]
    Unknown {
        host: String,
        source: std::io::Error,
    },
    #[error("cannot fetch {url}: {source}")]
    Fetch {
        url: String,
        source: Box<ureq::Error>,
    },
    #[error("cannot read {url}: {source}")]
    Read { url: String, source: std::io::Error },
    #[error("{url} answered {status}")]
    Status { url: String, status: u16 },
    #[error("{0} sends somewhere it cannot be followed")]
    Redirect(String),
    #[error("{0} is larger than a preview may be")]
    TooLarge(String),
    #[error("{0} has nothing to show")]
    Empty(String),
    #[error(transparent)]
    Picture(#[from] PictureError),
}

pub trait Previews: Send + Sync {
    fn preview(&self, url: &Url) -> Result<LinkPreview, PreviewError>;
}

pub struct Web {
    agent: ureq::Agent,
    reach: Reach,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    Internet,
    #[cfg(test)]
    ThisComputerToo,
}

impl Reach {
    fn takes_this_computer(self) -> bool {
        match self {
            Reach::Internet => false,
            #[cfg(test)]
            Reach::ThisComputerToo => true,
        }
    }
}

impl Default for Web {
    fn default() -> Self {
        Self::with_reach(Reach::Internet)
    }
}

impl Web {
    fn with_reach(reach: Reach) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(concat!(
                "Mozilla/5.0 (compatible; Whisperia/",
                env!("CARGO_PKG_VERSION"),
                "; +",
                env!("CARGO_PKG_REPOSITORY"),
                ")"
            ))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(agent),
            reach,
        }
    }

    fn fetch(
        &self,
        mut at: url::Url,
        kinds: &str,
        most: u64,
        whole: bool,
    ) -> Result<(url::Url, Vec<u8>), PreviewError> {
        for _ in 0..=MOST_REDIRECTS {
            self.check(&at)?;
            let mut response = self
                .agent
                .get(at.as_str())
                .header("Accept", kinds)
                .call()
                .map_err(|source| PreviewError::Fetch {
                    url: at.to_string(),
                    source: Box::new(source),
                })?;
            let status = response.status();
            if status.is_redirection() {
                let next = response
                    .headers()
                    .get("location")
                    .and_then(|location| location.to_str().ok())
                    .and_then(|location| at.join(location).ok())
                    .ok_or_else(|| PreviewError::Redirect(at.to_string()))?;
                at = web(next)?;
                continue;
            }
            if !status.is_success() {
                return Err(PreviewError::Status {
                    url: at.to_string(),
                    status: status.as_u16(),
                });
            }
            let mut bytes = Vec::new();
            response
                .body_mut()
                .as_reader()
                .take(most + 1)
                .read_to_end(&mut bytes)
                .map_err(|source| PreviewError::Read {
                    url: at.to_string(),
                    source,
                })?;
            if bytes.len() as u64 > most {
                if whole {
                    return Err(PreviewError::TooLarge(at.to_string()));
                }
                bytes.truncate(most as usize);
            }
            return Ok((at, bytes));
        }
        Err(PreviewError::Redirect(at.to_string()))
    }

    fn check(&self, at: &url::Url) -> Result<(), PreviewError> {
        if self.reach.takes_this_computer() {
            return Ok(());
        }
        let host = at
            .host_str()
            .ok_or_else(|| PreviewError::NotWeb(at.to_string()))?;
        let port = at.port_or_known_default().unwrap_or(443);
        let addresses: Vec<IpAddr> = (host.trim_matches(['[', ']']), port)
            .to_socket_addrs()
            .map_err(|source| PreviewError::Unknown {
                host: host.to_owned(),
                source,
            })?
            .map(|address| address.ip())
            .collect();
        if addresses.is_empty() || !addresses.iter().all(|ip| is_public(*ip)) {
            return Err(PreviewError::NotPublic(host.to_owned()));
        }
        Ok(())
    }
}

impl Previews for Web {
    fn preview(&self, url: &Url) -> Result<LinkPreview, PreviewError> {
        let start = url::Url::parse(url.as_str())
            .map_err(|_| PreviewError::NotWeb(url.to_string()))
            .and_then(web)?;
        let (landed, html) = self.fetch(start, PAGE_KINDS, MOST_PAGE_BYTES, false)?;
        let page = Page::read(&String::from_utf8_lossy(&html));
        if !page.has_anything() {
            return Err(PreviewError::Empty(url.to_string()));
        }
        let picture = page
            .picture
            .as_deref()
            .and_then(|picture| landed.join(picture).ok())
            .and_then(|picture| web(picture).ok())
            .and_then(|picture| {
                let shown = self
                    .fetch(picture, PICTURE_KINDS, MOST_PICTURE_BYTES, true)
                    .and_then(|(_, bytes)| Ok(Picture::decode(&bytes)?));
                shown
                    .inspect_err(|e| tracing::debug!("no picture for {url}: {e}"))
                    .ok()
            });
        Ok(LinkPreview {
            site: page.site.unwrap_or_else(|| url.site().to_owned()),
            title: page.title,
            description: page.description,
            picture,
            colour: page.colour,
        })
    }
}

fn web(at: url::Url) -> Result<url::Url, PreviewError> {
    match at.scheme() {
        "http" | "https" => Ok(at),
        _ => Err(PreviewError::NotWeb(at.to_string())),
    }
}

fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(mapped) => is_public_v4(mapped),
            None => is_public_v6(ip),
        },
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || a == 0
        || a >= 240
        || (a == 100 && (64..128).contains(&b))
        || (a == 198 && (18..20).contains(&b)))
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || first & 0xfe00 == 0xfc00
        || first & 0xffc0 == 0xfe80
        || first == 0x2001 && ip.segments()[1] == 0x0db8)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    use image::{ImageFormat, RgbaImage};

    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        RgbaImage::from_pixel(64, 32, image::Rgba([30, 36, 51, 255]))
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    enum Answer {
        Page(&'static str, Vec<u8>),
        Moved(&'static str),
    }

    fn serve(answers: Vec<(&'static str, Answer)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                let mut line = String::from("-");
                while !line.trim().is_empty() {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                }
                let path = first.split_whitespace().nth(1).unwrap_or_default();
                let head = match answers.iter().find(|(at, _)| *at == path) {
                    Some((_, Answer::Page(kind, body))) => {
                        format!(
                            "200 OK
Content-Type: {kind}
Content-Length: {}",
                            body.len()
                        )
                    }
                    Some((_, Answer::Moved(to))) => {
                        format!(
                            "301 Moved Permanently
Location: {to}
Content-Length: 0"
                        )
                    }
                    None => "404 Not Found
Content-Length: 0"
                        .to_owned(),
                };
                let body = match answers.iter().find(|(at, _)| *at == path) {
                    Some((_, Answer::Page(_, body))) => body.as_slice(),
                    _ => &[],
                };
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {head}
Connection: close

"
                    )
                    .as_bytes(),
                );
                let _ = stream.write_all(body);
            }
        });
        address
    }

    #[test]
    fn a_page_gives_its_title_description_and_picture() {
        let page = br#"<html><head>
            <meta property="og:title" content="A page">
            <meta property="og:description" content="About something">
            <meta property="og:image" content="/card.png">
            </head></html>"#;
        let address = serve(vec![
            ("/start", Answer::Moved("/page")),
            (
                "/page",
                Answer::Page("text/html; charset=utf-8", page.to_vec()),
            ),
            ("/card.png", Answer::Page("image/png", png())),
        ]);
        let web = Web::with_reach(Reach::ThisComputerToo);
        let preview = web.preview(&url(&format!("{address}/start"))).unwrap();
        assert_eq!(preview.site, address.trim_start_matches("http://"));
        assert_eq!(preview.title.as_deref(), Some("A page"));
        assert_eq!(preview.description.as_deref(), Some("About something"));
        let picture = preview.picture.unwrap();
        assert_eq!((picture.width(), picture.height()), (64, 32));
    }

    #[test]
    fn a_page_with_nothing_to_show_is_no_preview() {
        let address = serve(vec![(
            "/",
            Answer::Page("text/html", b"<html><body>hi</body></html>".to_vec()),
        )]);
        let web = Web::with_reach(Reach::ThisComputerToo);
        assert!(matches!(
            web.preview(&url(&format!("{address}/"))),
            Err(PreviewError::Empty(_))
        ));
    }

    #[test]
    fn a_missing_page_is_no_preview() {
        let address = serve(vec![]);
        let web = Web::with_reach(Reach::ThisComputerToo);
        assert!(matches!(
            web.preview(&url(&format!("{address}/gone"))),
            Err(PreviewError::Status { status: 404, .. })
        ));
    }

    #[test]
    fn this_computer_and_the_local_network_are_never_asked() {
        let web = Web::default();
        for address in [
            "http://127.0.0.1/",
            "http://localhost:8080/",
            "http://192.168.1.1/admin",
            "http://10.0.0.5/",
            "http://[::1]/",
            "http://169.254.169.254/latest/meta-data",
        ] {
            assert!(
                matches!(web.preview(&url(address)), Err(PreviewError::NotPublic(_))),
                "{address}"
            );
        }
    }

    #[test]
    fn only_addresses_on_the_internet_are_public() {
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700::1111", "::ffff:8.8.8.8"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "0.0.0.0",
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.0.1",
            "169.254.0.1",
            "100.64.0.1",
            "198.18.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "::ffff:192.168.0.1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
    }
}
