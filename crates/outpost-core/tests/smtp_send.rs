//! The SMTP client against a fake server that can misbehave.

mod common;

use common::smtp::{Behaviour, SmtpSink};
use common::PASSWORD;
use outpost_core::smtp::{send, SendError, SmtpSecurity, SmtpServer};

fn server(sink: &SmtpSink) -> SmtpServer {
    SmtpServer {
        host: "127.0.0.1".into(),
        port: sink.port,
        security: SmtpSecurity::Plaintext,
        trusted_certificate: None,
    }
}

const MESSAGE: &[u8] = b"From: Me <me@example.com>\r\nTo: You <you@example.com>\r\nSubject: hi\r\n\r\n\
First line.\r\n.\r\n..two dots\r\n.leading dot\r\nLast line.\r\n";

fn to(addresses: &[&str]) -> Vec<String> {
    addresses.iter().map(|a| a.to_string()).collect()
}

#[tokio::test]
async fn a_message_is_handed_over_intact() {
    let sink = SmtpSink::start();
    send(&server(&sink), "me@example.com", PASSWORD, "me@example.com", &to(&["you@example.com", "them@example.org"]), MESSAGE)
        .await
        .unwrap();
    let got = sink.messages();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].from, "me@example.com");
    assert_eq!(got[0].recipients, ["you@example.com", "them@example.org"]);
    assert_eq!(got[0].signed_in_as.as_deref(), Some("me@example.com"));
    assert_eq!(got[0].message, MESSAGE, "lines that are or start with a dot arrive unchanged");

    // The older LOGIN way of signing in works too.
    sink.behave(Behaviour::LoginOnly);
    send(&server(&sink), "me@example.com", PASSWORD, "me@example.com", &to(&["you@example.com"]), b"Subject: x\n\nbare newlines\n")
        .await
        .unwrap();
    let got = sink.messages();
    assert_eq!(got[1].signed_in_as.as_deref(), Some("me@example.com"));
    assert_eq!(got[1].message, b"Subject: x\r\n\r\nbare newlines\r\n");
}

#[tokio::test]
async fn each_kind_of_failure_is_told_apart() {
    let sink = SmtpSink::start();
    let attempt = |password: &'static str, recipients: Vec<String>| {
        let server = server(&sink);
        async move { send(&server, "me@example.com", password, "me@example.com", &recipients, MESSAGE).await }
    };

    // A wrong password: no, and retrying will not help.
    match attempt("wrong", to(&["you@example.com"])).await {
        Err(SendError::Refused(text)) => assert!(text.contains("sign-in failed"), "{text}"),
        other => panic!("{other:?}"),
    }
    // A recipient the server will not take: no, naming which one, and the
    // message goes to nobody rather than to some of the recipients.
    match attempt(PASSWORD, to(&["you@example.com", "nobody@example.com"])).await {
        Err(SendError::Refused(text)) => assert!(text.contains("nobody@example.com"), "{text}"),
        other => panic!("{other:?}"),
    }
    // No recipients, or an address that could smuggle in a command.
    assert!(matches!(attempt(PASSWORD, vec![]).await, Err(SendError::Refused(_))));
    assert!(matches!(attempt(PASSWORD, to(&["a@b.c>\r\nRCPT TO:<evil@example.com"])).await, Err(SendError::Refused(_))));
    // The server refuses the message itself.
    sink.behave(Behaviour::RefuseMessage);
    assert!(matches!(attempt(PASSWORD, to(&["you@example.com"])).await, Err(SendError::Refused(_))));
    assert!(sink.messages().is_empty(), "none of those was sent");

    // Busy, or not there at all: not sent, worth trying again.
    sink.behave(Behaviour::Busy);
    assert!(matches!(attempt(PASSWORD, to(&["you@example.com"])).await, Err(SendError::Unavailable(_))));
    let nowhere = SmtpServer { port: 1, ..server(&sink) };
    assert!(matches!(
        send(&nowhere, "me@example.com", PASSWORD, "me@example.com", &to(&["you@example.com"]), MESSAGE).await,
        Err(SendError::Unavailable(_))
    ));

    // The connection is lost after the message was handed over: unknown.
    // (This fake server did keep it, which is exactly why a blind retry
    // would be wrong.)
    sink.behave(Behaviour::VanishAfterMessage);
    assert!(matches!(attempt(PASSWORD, to(&["you@example.com"])).await, Err(SendError::Uncertain(_))));
    assert_eq!(sink.messages().len(), 1);

    // A server that will not encrypt is not used when encryption was asked for.
    sink.behave(Behaviour::Normal);
    let demanding = SmtpServer { security: SmtpSecurity::StartTls, ..server(&sink) };
    match send(&demanding, "me@example.com", PASSWORD, "me@example.com", &to(&["you@example.com"]), MESSAGE).await {
        Err(SendError::Refused(text)) => assert!(text.contains("encryption"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(sink.messages().len(), 1, "nothing was sent unencrypted");
}
