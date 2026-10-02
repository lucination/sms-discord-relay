use std::{net::SocketAddr, time::Duration};

fn target_address(bind: Option<&str>) -> Result<SocketAddr, ()> {
    let mut address: SocketAddr = bind.unwrap_or("0.0.0.0:8080").parse().map_err(|_| ())?;
    if address.port() == 0 {
        return Err(());
    }
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            std::net::Ipv4Addr::LOCALHOST.into()
        } else {
            std::net::Ipv6Addr::LOCALHOST.into()
        });
    }
    Ok(address)
}

pub async fn run() -> Result<(), ()> {
    let bind = match std::env::var("BIND_ADDR") {
        Ok(bind) => Some(bind),
        Err(std::env::VarError::NotPresent) => None,
        Err(_) => return Err(()),
    };
    let address = target_address(bind.as_deref())?;
    let response = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(2))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| ())?
        .get(format!("http://{address}/healthz"))
        .send()
        .await
        .map_err(|_| ())?;
    if response.status() == reqwest::StatusCode::OK {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_bind_defaults_to_ipv4_loopback_port_8080() {
        assert_eq!(target_address(None).unwrap().to_string(), "127.0.0.1:8080");
    }

    #[test]
    fn invalid_addresses_and_zero_ports_are_rejected() {
        for bind in [
            "127.0.0.1:0",
            "[::1]:0",
            "localhost:8080",
            "",
            "bad-secret",
            "127.0.0.1:8080/path",
            "http://127.0.0.1:8080",
        ] {
            assert!(target_address(Some(bind)).is_err(), "accepted {bind}");
        }
    }

    #[test]
    fn wildcard_addresses_use_matching_loopback_family_and_preserve_port() {
        for (bind, expected) in [
            ("0.0.0.0:8080", "127.0.0.1:8080"),
            ("[::]:9123", "[::1]:9123"),
            ("127.0.0.2:4567", "127.0.0.2:4567"),
            ("[2001:db8::42]:5678", "[2001:db8::42]:5678"),
        ] {
            assert_eq!(target_address(Some(bind)).unwrap().to_string(), expected);
        }
    }
}
