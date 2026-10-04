use std::time::Duration;

use whisperia_e2e::{fakes, gui::*, TestServer};
use whisperia_proto::PROTOCOL_VERSION;

const FOUND: Duration = Duration::from_secs(25);

fn on_the_lan(name: &str) -> Window {
    opening(named(name)).lan().open()
}

#[test]
#[ignore = "announces over real mDNS on the network"]
fn a_server_on_the_network_is_listed_and_a_click_joins_it() {
    let name = format!("DESKTOP-{:X}", std::process::id());
    let server = TestServer::named(&name).advertised().start();
    let connect = format!("Connect to {name}");

    let mut panda = on_the_lan("Panda");
    wait_for_within(&mut panda, &connect, FOUND);
    press(&mut panda, &connect);
    wait_for(&mut panda, "Nobody is in a room yet");
    press(&mut panda, "Open");
    wait_for(&mut panda, "Panda (you)");

    let mut fox = on_the_lan("Fox");
    wait_until_within(&mut fox, &format!("{name} online"), FOUND, |ui| {
        ui.texts_within(&connect)
            .iter()
            .any(|text| text == "Online")
    });
    drop(server);
}

#[test]
#[ignore = "announces over real mDNS on the network"]
fn a_server_announced_with_a_newer_version_asks_to_update_this_pc() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _in_runtime = runtime.enter();
    let (_newer, newer_at) = fakes::server_from_before_peeks();
    let _announced = fakes::announce("DESKTOP-R8ZE", PROTOCOL_VERSION + 1, newer_at.port());

    let mut panda = on_the_lan("Panda");
    wait_for_within(&mut panda, "Connect to DESKTOP-R8ZE", FOUND);
    press(&mut panda, "Connect to DESKTOP-R8ZE");
    wait_for(
        &mut panda,
        &format!(
            "\"DESKTOP-R8ZE\" has Whisperia {}, this PC has {PROTOCOL_VERSION}. Update this PC.",
            PROTOCOL_VERSION + 1
        ),
    );
    picture(&panda, "40-another-version");
    press(&mut panda, "OK");
    wait_until_gone(&mut panda, "Update this PC");
}

#[test]
#[ignore = "announces over real mDNS on the network"]
fn a_server_still_announced_but_switched_off_leaves_the_list() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _in_runtime = runtime.enter();
    let name = format!("DESKTOP-G{:X}", std::process::id());
    let mut up = TestServer::named(&name).on_every_address().start();
    let _announced = fakes::announce(&name, PROTOCOL_VERSION, up.port());

    let mut panda = on_the_lan("Panda");
    let connect = format!("Connect to {name}");
    wait_for_within(&mut panda, &connect, FOUND);
    wait_until(&mut panda, &format!("{name} online"), |ui| {
        ui.texts_within(&connect)
            .iter()
            .any(|text| text == "Online")
    });

    up.stop();
    wait_until_gone_within(&mut panda, &name, Duration::from_secs(30));
}
