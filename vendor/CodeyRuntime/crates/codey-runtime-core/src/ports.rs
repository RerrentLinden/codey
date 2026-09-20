use std::{io, net::TcpListener};

/// Fallible selection for callers that cannot launch with an unknown debug port.
pub fn try_select_packaged_codex_debug_port_with(
    requested: u16,
    is_windows: bool,
    can_bind: impl Fn(u16) -> bool,
    find_available: impl Fn() -> io::Result<u16>,
) -> io::Result<u16> {
    let selected = if !is_windows || can_bind(requested) {
        requested
    } else {
        find_available()?
    };
    if selected == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "debug port selection returned port 0",
        ));
    }
    Ok(selected)
}

pub fn can_bind_loopback_port(port: u16) -> bool {
    if port == 0 {
        return true;
    }
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

pub fn try_find_available_loopback_port() -> io::Result<u16> {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_windows_keeps_requested_debug_port_even_when_busy() {
        assert_eq!(
            try_select_packaged_codex_debug_port_with(9229, false, |_| false, || Ok(1)).unwrap(),
            9229
        );
    }

    #[test]
    fn windows_falls_back_to_an_available_port_when_requested_is_busy() {
        assert_eq!(
            try_select_packaged_codex_debug_port_with(9229, true, |_| false, || Ok(4321)).unwrap(),
            4321
        );
        assert_eq!(
            try_select_packaged_codex_debug_port_with(9229, true, |_| true, || Ok(4321)).unwrap(),
            9229
        );
    }

    #[test]
    fn occupied_loopback_port_selects_a_nonzero_available_fallback() {
        let occupied = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let requested = occupied.local_addr().unwrap().port();
        let selected = try_select_packaged_codex_debug_port_with(
            requested,
            true,
            can_bind_loopback_port,
            try_find_available_loopback_port,
        )
        .unwrap();
        assert_ne!(selected, 0);
        assert_ne!(selected, requested);
        assert!(can_bind_loopback_port(selected));
    }

    #[test]
    fn fallback_probe_failure_preserves_the_io_error() {
        let error = try_select_packaged_codex_debug_port_with(
            9229,
            true,
            |_| false,
            || {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "bind denied",
                ))
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "bind denied");
    }

    #[test]
    fn fallible_selection_rejects_zero_from_request_or_fallback() {
        for (requested, can_bind) in [(0, true), (9229, false)] {
            assert!(
                try_select_packaged_codex_debug_port_with(requested, true, |_| can_bind, || Ok(0),)
                    .is_err()
            );
        }
    }

    #[test]
    fn available_requested_port_does_not_probe_fallback() {
        assert_eq!(
            try_select_packaged_codex_debug_port_with(
                9229,
                true,
                |_| true,
                || panic!("fallback is unnecessary"),
            )
            .unwrap(),
            9229
        );
    }
}
