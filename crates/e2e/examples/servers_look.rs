use std::{net::SocketAddr, sync::Arc, time::Duration};

use whisperia::{
    audio::fake::FakeBackend,
    config::{Config, Paths, RecentServer},
    discovery::Search,
    gui,
    launch::Launched,
    net::Session,
    preview::fake::FakePreviews,
    screen::fake::FakeScreen,
    wake::Waker,
};
use whisperia_e2e::{fakes, gui::named, net, TempDir, TestServer};
use whisperia_proto::{control::Name, PROTOCOL_VERSION};

fn main() {
    let dir = TempDir::new("servers-look");
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");
    let _in_runtime = runtime.enter();

    let busy = TestServer::named("DESKTOP-AV83").advertised().start();
    let _empty = TestServer::named("DESKTOP-6TNC").advertised().start();
    let quiet = TestServer::start("DESKTOP-9FJD");

    let person = |addr: SocketAddr, name: &str, room: &str| -> (Session, net::Events) {
        let (session, _, events) = runtime.block_on(net::connect(addr, name));
        net::enter(&session, room);
        (session, events)
    };
    let _people = [
        person(busy.addr(), "Fox", "General"),
        person(busy.addr(), "Mouse", "General"),
        person(busy.addr(), "Raccoon", "Backroom"),
        person(quiet.addr(), "Penguin", "Room 83"),
    ];

    let (_old, old) = fakes::server_from_before_peeks();
    let (_newer, newer) = fakes::server_from_before_peeks();
    let _announced = fakes::announce("DESKTOP-R8ZE", PROTOCOL_VERSION + 1, newer.port());
    let (_slow, slow) = fakes::slow_server("DESKTOP-HB21", Duration::from_millis(4500));
    let nobody = std::net::UdpSocket::bind("127.0.0.1:0").expect("a port nobody answers on");

    let recent = |name: &str, addr: SocketAddr| RecentServer {
        name: Name::new(name).expect("a name"),
        address: addr.into(),
        id: None,
    };
    let nobody_at = nobody.local_addr().expect("the silent port");
    gui::run(
        gui::Options {
            paths: Paths::resolve(Some(dir.path().to_owned())).expect("the settings folder"),
            config: Config {
                recent: vec![
                    recent("DESKTOP-9FJD", quiet.addr()),
                    recent("DESKTOP-HB21", slow),
                    recent("DESKTOP-P4LW", old),
                    recent("DESKTOP-3MRX", nobody_at),
                ],
                ..named("Panda")
            },
            backend: Arc::new(FakeBackend::quiet(48_000, None)),
            screen: Arc::new(FakeScreen::new()),
            previews: Arc::new(FakePreviews::default()),
            connect: None,
            desktop: None,
            instance: None,
            waker: Waker::default(),
            search: Search::Lan,
        },
        &Launched::default(),
    )
    .expect("the window opens");
}
