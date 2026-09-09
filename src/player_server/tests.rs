use super::*;
fn parse(text: &str) -> bool {
    request(&mut text.as_bytes(), 8765, None).is_ok()
}
#[test]
fn http_rejects_cross_origin_and_oversized_requests() {
    assert!(parse("GET / HTTP/1.1\r\nHost: 127.0.0.1:8765\r\n\r\n"));
    for headers in [
        "Host: evil.example",
        "Host: localhost:8765\r\nOrigin: http://evil.example",
        "Host: localhost:8765\r\nContent-Length: 129",
        "Host: localhost:8765\r\nTransfer-Encoding: chunked",
        "Host: localhost:8765\r\nHost: localhost:8765",
    ] {
        assert!(!parse(&format!("GET / HTTP/1.1\r\n{headers}\r\n\r\n")));
    }
    assert!(!parse(
        "POST /api/move HTTP/1.1\r\nHost: localhost:8765\r\n\r\n"
    ));
    assert!(parse("POST /api/reset HTTP/1.1\r\nHost: localhost:8765\r\nOrigin: http://localhost:8765\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}"));
}
#[test]
fn reset_cancels_an_old_practice_reply() {
    let app = Arc::new(Mutex::new(App::new()));
    let send = |path: &str, body: Value| {
        route(
            Request {
                method: "POST".into(),
                path: path.into(),
                body: body.to_string().into_bytes(),
                cookie: None,
            },
            &app,
        )
    };
    send("/api/reset", json!({"revision":0,"mode":"play-first"})).unwrap();
    send("/api/move", json!({"revision":1,"square":14})).unwrap();
    let old_cancel = app.lock().cancel.clone();
    assert!(app.lock().busy);
    send("/api/reset", json!({"revision":2,"mode":"play-first"})).unwrap();
    assert!(old_cancel.load(Ordering::Relaxed));
    assert!(!app.lock().busy);
    assert_eq!(app.lock().state()["moves"], json!([]));
    assert!(send("/api/step", json!({"revision":3})).is_err());
}
#[test]
fn public_visitors_have_separate_games_and_expired_sessions_are_cancelled() {
    let hub = Hub {
        template: Arc::new(Mutex::new(App::new())),
        visitors: Mutex::new(HashMap::new()),
        origin_file: None,
    };
    let (id, fresh, a) = hub.visitor(None).unwrap();
    assert!(fresh);
    assert_eq!(id.len(), 48);
    let (other, _, b) = hub.visitor(None).unwrap();
    assert_ne!(id, other);
    a.lock().game = Some(Session::Practice(crate::trainer::Trainer::new(
        crate::rules::Rules::default(),
    )));
    assert_eq!(a.lock().state()["mode"], "play-first");
    assert_eq!(b.lock().state()["mode"], "verified");
    let (_, fresh, again) = hub
        .visitor(Some(&format!("__Host-gekitai_session={id}")))
        .unwrap();
    assert!(!fresh);
    assert!(Arc::ptr_eq(&a, &again));
    hub.visitors.lock().get_mut(&id).unwrap().last_seen =
        Instant::now() - Duration::from_secs(3601);
    let (replacement, fresh, _) = hub
        .visitor(Some(&format!("__Host-gekitai_session={id}")))
        .unwrap();
    assert!(fresh);
    assert_ne!(id, replacement);
    assert!(a.lock().cancel.load(Ordering::Relaxed));
}
#[test]
fn only_the_configured_https_origin_is_accepted() {
    let body="POST /api/reset HTTP/1.1\r\nHost: localhost:8765\r\nOrigin: https://game.example.com\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
    assert!(request(&mut body.as_bytes(), 8765, Some("https://game.example.com")).is_ok());
    assert!(request(&mut body.as_bytes(), 8765, None).is_err());
    assert!(request(
        &mut body.as_bytes(),
        8765,
        Some("https://other.example.com")
    )
    .is_err());
}
#[test]
fn busy_capacity_rejects_before_changing_a_board() {
    let mut app = App::new();
    app.game = Some(Session::Practice(crate::trainer::Trainer::new(
        crate::rules::Rules::default(),
    )));
    let permits: Vec<_> = (0..4)
        .map(|_| AnalysisPermit::reserve(app.analysis_slots.clone()).unwrap())
        .collect();
    let before = app.state();
    let app = Arc::new(Mutex::new(app));
    let result = route(
        Request {
            method: "POST".into(),
            path: "/api/move".into(),
            body: br#"{"revision":0,"square":14}"#.to_vec(),
            cookie: None,
        },
        &app,
    );
    assert!(result.is_err());
    assert_eq!(app.lock().state(), before);
    drop(permits);
    assert_eq!(app.lock().analysis_slots.load(Ordering::Relaxed), 0);
}
#[test]
fn waiting_and_stale_requests_cannot_move() {
    let app = Arc::new(Mutex::new(App::new()));
    app.lock().revision = 3;
    for body in [
        r#"{"revision":2,"square":0}"#,
        r#"{"revision":3,"square":0}"#,
    ] {
        assert!(route(
            Request {
                method: "POST".into(),
                path: "/api/move".into(),
                body: body.as_bytes().to_vec(),
                cookie: None
            },
            &app
        )
        .is_err());
    }
    assert_eq!(app.lock().revision, 3);
}
