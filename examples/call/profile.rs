// SPDX-License-Identifier: MIT
use super::transport;
use std::{
    io,
    net::{SocketAddr, ToSocketAddrs},
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Family {
    #[default]
    Auto,
    Ipv4,
    Ipv6,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::Ipv4 => "IPv4",
            Self::Ipv6 => "IPv6",
        }
    }
    pub fn cycle(self, backwards: bool) -> Self {
        match (self, backwards) {
            (Self::Auto, false) | (Self::Ipv6, true) => Self::Ipv4,
            (Self::Ipv4, false) | (Self::Auto, true) => Self::Ipv6,
            _ => Self::Auto,
        }
    }
    fn accepts(self, address: SocketAddr) -> bool {
        match self {
            Self::Auto => true,
            Self::Ipv4 => address.is_ipv4(),
            Self::Ipv6 => address.is_ipv6(),
        }
    }
}

/// Kept only in memory. Never derive Debug or print the shared room key.
#[derive(Clone)]
pub struct Profile {
    pub remote: String,
    pub bind: String,
    pub direct: bool,
    pub role: u8,
    pub family: Family,
    pub key: String,
}

impl Profile {
    pub fn addresses(&self) -> io::Result<(SocketAddr, SocketAddr)> {
        if !(1..=2).contains(&self.role) {
            return Err(io::Error::other(
                "Choose role 1 or 2; the other client must use a different role",
            ));
        }
        let explicit = if self.bind.trim().is_empty() {
            None
        } else {
            Some(self.bind.trim().parse::<SocketAddr>().map_err(|_| {
                io::Error::other(
                    "Local bind must be IP:PORT or [IPv6]:PORT, or empty for automatic",
                )
            })?)
        };
        if explicit.is_some_and(|address| !self.family.accepts(address)) {
            return Err(io::Error::other(
                "Local bind conflicts with the selected IP family",
            ));
        }
        if self.remote.trim().is_empty() {
            return Err(io::Error::other(
                "Enter a peer or relay address: HOST:PORT or [IPv6]:PORT",
            ));
        }
        let remote = self
            .remote
            .trim()
            .to_socket_addrs()
            .map_err(|_| {
                io::Error::other("Cannot resolve remote address; use HOST:PORT or [IPv6]:PORT")
            })?
            .find(|r| {
                self.family.accepts(*r) && explicit.is_none_or(|l| r.is_ipv6() == l.is_ipv6())
            })
            .ok_or_else(|| {
                io::Error::other("No remote address matches the IP family and local bind")
            })?;
        if remote.port() == 0 || remote.ip().is_unspecified() || remote.ip().is_multicast() {
            return Err(io::Error::other(
                "Remote address must identify one host and a nonzero UDP port",
            ));
        }
        let local = explicit.unwrap_or_else(|| {
            let port = if self.direct { 5000 } else { 0 };
            if remote.is_ipv6() {
                SocketAddr::from(([0u16; 8], port))
            } else {
                SocketAddr::from(([0u8; 4], port))
            }
        });
        Ok((local, remote))
    }

    pub fn mode(&self, remote: SocketAddr) -> &'static str {
        match (self.direct, remote.is_ipv6()) {
            (true, false) => "DIRECT IPv4",
            (true, true) => "DIRECT IPv6",
            (false, false) => "RELAY IPv4",
            (false, true) => "RELAY IPv6",
        }
    }

    pub fn key_bytes(&self) -> io::Result<[u8; 32]> {
        transport::parse_key(self.key.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile(remote: &str, family: Family) -> Profile {
        Profile {
            remote: remote.into(),
            bind: String::new(),
            role: 1,
            direct: true,
            family,
            key: "ab".repeat(32),
        }
    }
    #[test]
    fn selects_ipv4_and_ipv6_and_default_ports_for_both_modes() {
        for (remote, family, local) in [
            ("127.0.0.1:5001", Family::Ipv4, "0.0.0.0:5000"),
            ("[::1]:5001", Family::Ipv6, "[::]:5000"),
        ] {
            let mut profile = profile(remote, family);
            assert_eq!(profile.addresses().unwrap().0, local.parse().unwrap());
            profile.direct = false;
            assert_eq!(profile.addresses().unwrap().0.port(), 0);
        }
        assert!(
            profile("localhost:5001", Family::Ipv4)
                .addresses()
                .unwrap()
                .1
                .is_ipv4()
        );
        assert!(
            profile("localhost:5001", Family::Ipv6)
                .addresses()
                .unwrap()
                .1
                .is_ipv6()
        );
    }
    #[test]
    fn rejects_incompatible_families_and_unusable_endpoints() {
        for (remote, family) in [
            ("[::1]:5001", Family::Ipv4),
            ("127.0.0.1:5001", Family::Ipv6),
            ("127.0.0.1:0", Family::Auto),
            ("0.0.0.0:5000", Family::Auto),
            ("224.0.0.1:5000", Family::Auto),
            ("", Family::Auto),
        ] {
            assert!(profile(remote, family).addresses().is_err());
        }
        let mut profile = profile("127.0.0.1:5001", Family::Auto);
        profile.bind = "[::]:5000".into();
        assert!(profile.addresses().is_err());
        profile.family = Family::Ipv4;
        assert!(profile.addresses().is_err());
    }
}
