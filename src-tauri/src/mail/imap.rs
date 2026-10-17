//! SECURITY-RELEVANT: this is where TLS for IMAP is set up. Implicit TLS
//! only (port 993 style), strict certificate validation — never add any
//! danger_accept_invalid_* call here.

use std::time::Duration;

use async_imap::extensions::idle::IdleResponse;
use async_imap::imap_proto;
use async_imap::types::{Fetch, Flag, Name, NameAttribute};
use async_imap::Session;
use async_native_tls::TlsStream;
use futures::TryStreamExt;
use tokio::net::TcpStream;

use crate::auth::Credential;
use crate::error::AppError;
use crate::storage::mailboxes::DiscoveredMailbox;
use crate::timing;

pub type ImapSession = Session<TlsStream<TcpStream>>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
// why BODYSTRUCTURE: it rides along for free and tells whether a message
// carries attachments before any body is downloaded (the list's paperclip).
const HEADER_QUERY: &str = "(UID FLAGS BODYSTRUCTURE BODY.PEEK[HEADER])";

/// One fetched message header, still raw — parsing lives in mail::parse.
#[derive(Debug)]
pub struct RawHeader {
    pub uid: i64,
    pub read: bool,
    /// From BODYSTRUCTURE — an approximation refined by the body parse.
    pub has_attachments: bool,
    pub header: Vec<u8>,
}

/// TLS connect + LOGIN. The timeout covers the whole handshake so a black-
/// holed server can't hang a sync forever.
pub async fn connect(
    host: &str,
    port: u16,
    username: &str,
    credential: &Credential,
) -> Result<ImapSession, AppError> {
    let _t = timing::start("imap::connect");
    tokio::time::timeout(CONNECT_TIMEOUT, async {
        let tcp = TcpStream::connect((host, port))
            .await
            .map_err(|e| AppError::Imap(format!("connect {host}:{port}: {e}")))?;
        let tls = async_native_tls::TlsConnector::new()
            .connect(host, tcp)
            .await
            .map_err(|e| AppError::Imap(format!("tls handshake: {e}")))?;
        log_in(tls, username, credential).await
    })
    .await
    .map_err(|_| AppError::Imap(format!("connection to {host}:{port} timed out")))?
}

/// LOGIN, or AUTHENTICATE for a token, over a connection already open.
/// why generic over the stream: a test can drive it over an in-memory pipe
/// standing in for the server, without a network or TLS.
async fn log_in<T>(
    stream: T,
    username: &str,
    credential: &Credential,
) -> Result<Session<T>, AppError>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let mut client = async_imap::Client::new(stream);
    // why: async-imap leaves the server's greeting unread. LOGIN never
    // noticed — it skips untagged lines on its way to the tagged reply — but
    // AUTHENTICATE takes the greeting for the server's first answer, then
    // skips the "+" prompt that follows and waits for a reply the server is
    // itself waiting on us for, until the timeout.
    client
        .read_response()
        .await
        .map_err(|e| AppError::Imap(format!("greeting: {e}")))?
        .ok_or_else(|| AppError::Imap("the server closed before its greeting".to_string()))?;
    match credential {
        Credential::Password(password) => client.login(username, password).await,
        Credential::AccessToken(token) => {
            client
                .authenticate("XOAUTH2", XOAuth2::new(username, token))
                .await
        }
    }
    .map_err(|(e, _)| AppError::Imap(format!("login: {e}")))
}

/// SASL XOAUTH2, the OAuth login Gmail and Outlook accept over IMAP: one
/// response carrying the user and a bearer token.
struct XOAuth2<'a> {
    user: &'a str,
    access_token: &'a str,
    sent: bool,
}

impl<'a> XOAuth2<'a> {
    fn new(user: &'a str, access_token: &'a str) -> Self {
        Self {
            user,
            access_token,
            sent: false,
        }
    }
}

impl async_imap::Authenticator for XOAuth2<'_> {
    type Response = String;

    fn process(&mut self, _challenge: &[u8]) -> String {
        // why: a server that rejects the token answers with one more
        // challenge (a JSON error) and expects an empty reply before its
        // final NO — sending the token again would be a protocol error.
        if std::mem::replace(&mut self.sent, true) {
            return String::new();
        }
        format!(
            "user={}\x01auth=Bearer {}\x01\x01",
            self.user, self.access_token
        )
    }
}

/// Connection check for the add-account flow: connect, LOGIN, LOGOUT.
pub async fn verify(
    host: &str,
    port: u16,
    username: &str,
    credential: &Credential,
) -> Result<(), AppError> {
    let mut session = connect(host, port, username, credential).await?;
    // why: best effort — the credentials were already proven by LOGIN.
    let _ = session.logout().await;
    Ok(())
}

/// Fetch headers by sequence-number range (initial sync).
pub async fn fetch_headers_by_seq(
    session: &mut ImapSession,
    range: &str,
) -> Result<Vec<RawHeader>, AppError> {
    let stream = session.fetch(range, HEADER_QUERY).await.map_err(imap_err)?;
    let fetches: Vec<Fetch> = stream.try_collect().await.map_err(imap_err)?;
    Ok(fetches.iter().filter_map(raw_header).collect())
}

/// Fetch headers by UID range (incremental sync).
pub async fn fetch_headers_by_uid(
    session: &mut ImapSession,
    uid_range: &str,
) -> Result<Vec<RawHeader>, AppError> {
    let stream = session
        .uid_fetch(uid_range, HEADER_QUERY)
        .await
        .map_err(imap_err)?;
    let fetches: Vec<Fetch> = stream.try_collect().await.map_err(imap_err)?;
    Ok(fetches.iter().filter_map(raw_header).collect())
}

/// All selectable folders on the server, with their special-use roles.
pub async fn list_mailboxes(session: &mut ImapSession) -> Result<Vec<DiscoveredMailbox>, AppError> {
    let _t = timing::start("imap::list_mailboxes");
    let stream = session.list(Some(""), Some("*")).await.map_err(imap_err)?;
    let names: Vec<Name> = stream.try_collect().await.map_err(imap_err)?;
    Ok(names
        .iter()
        .filter_map(|n| discovered(n.name(), n.attributes()))
        .collect())
}

/// Map one LIST line to a folder the app can use; `None` for containers
/// that can't hold mail (\Noselect). SPECIAL-USE attributes (RFC 6154) win;
/// well-known English names are the fallback for servers without them.
fn discovered(name: &str, attributes: &[NameAttribute]) -> Option<DiscoveredMailbox> {
    if attributes.contains(&NameAttribute::NoSelect) {
        return None;
    }
    let role = attributes
        .iter()
        .find_map(|attr| match attr {
            NameAttribute::Drafts => Some("drafts"),
            NameAttribute::Sent => Some("sent"),
            NameAttribute::Archive => Some("archive"),
            // why: Gmail has no \Archive folder — its "All Mail" (\All) is the
            // archive target, so moving there removes the Inbox label.
            NameAttribute::All => Some("all"),
            NameAttribute::Junk => Some("junk"),
            NameAttribute::Trash => Some("trash"),
            _ => None,
        })
        .or_else(|| role_from_name(name));
    Some(DiscoveredMailbox {
        name: name.to_string(),
        role: role.map(str::to_string),
    })
}

fn role_from_name(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "inbox" => Some("inbox"),
        "drafts" => Some("drafts"),
        "sent" | "sent messages" | "sent items" => Some("sent"),
        "archive" => Some("archive"),
        "junk" | "spam" => Some("junk"),
        "trash" | "deleted messages" => Some("trash"),
        _ => None,
    }
}

/// Whether the server offers IDLE (RFC 2177). Servers without it fall back
/// to interval polling; an unreadable CAPABILITY reply counts as "no".
pub async fn supports_idle(session: &mut ImapSession) -> bool {
    session
        .capabilities()
        .await
        .is_ok_and(|caps| caps.has_str("IDLE"))
}

/// What ended one IDLE wait.
#[derive(Debug, PartialEq, Eq)]
pub enum Idled {
    /// The server announced activity in the selected mailbox — sync it.
    Activity,
    /// Nothing happened before `limit`. Not an error: RFC 2177 requires the
    /// client to re-issue IDLE periodically anyway, or middleboxes silently
    /// drop the connection.
    Quiet,
}

/// Wait on the selected mailbox until the server says something or `limit`
/// passes, then leave IDLE and hand the session back.
///
/// why by value: `Session::idle` consumes the session and `done` returns it,
/// so ownership has to travel through the wait. A caller that drops the
/// returned session simply closes the connection.
pub async fn idle_once(
    session: ImapSession,
    limit: Duration,
) -> Result<(ImapSession, Idled), AppError> {
    let mut handle = session.idle();
    handle.init().await.map_err(imap_err)?;

    let outcome = {
        // The StopSource would interrupt the wait early; nothing here needs
        // that, and dropping it must not cancel the wait, so it is held for
        // exactly as long as the future is.
        let (wait, _interrupt) = handle.wait_with_timeout(limit);
        match wait.await.map_err(imap_err)? {
            IdleResponse::NewData(_) => Idled::Activity,
            IdleResponse::Timeout | IdleResponse::ManualInterrupt => Idled::Quiet,
        }
    };

    let session = handle.done().await.map_err(imap_err)?;
    Ok((session, outcome))
}

/// The sequence-number range covering the newest `window` messages of a
/// folder holding `exists` of them; `None` for an empty folder, where there
/// is nothing to sweep (and `FETCH 1:*` errors). `window` of `None` — or one
/// at least as large as the folder — covers everything.
///
/// why sequence numbers work here: they are contiguous 1..=exists, and a
/// server assigns UIDs in ascending order (RFC 3501 §2.3.1.1), so the
/// highest `window` sequence numbers are exactly the highest `window` UIDs.
pub fn sweep_range(exists: u32, window: Option<u32>) -> Option<String> {
    if exists == 0 {
        return None;
    }
    match window {
        Some(window) if window > 0 && window < exists => Some(format!("{}:*", exists - window + 1)),
        _ => Some("1:*".to_string()),
    }
}

/// One `(uid, seen)` sweep of `range` in the selected folder — numbers only,
/// no content — so reconciliation can spot deletions and flag changes made
/// by other clients. Build `range` with `sweep_range`.
pub async fn fetch_uid_flags(
    session: &mut ImapSession,
    range: &str,
) -> Result<Vec<(i64, bool)>, AppError> {
    let stream = session
        .fetch(range, "(UID FLAGS)")
        .await
        .map_err(imap_err)?;
    let fetches: Vec<Fetch> = stream.try_collect().await.map_err(imap_err)?;
    Ok(fetches
        .iter()
        .filter_map(|f| {
            Some((
                i64::from(f.uid?),
                f.flags().any(|flag| matches!(flag, Flag::Seen)),
            ))
        })
        .collect())
}

/// RFC822.SIZE for a set of UIDs in one round trip → `(uid, bytes)` pairs.
/// UIDs the server no longer knows simply don't come back.
pub async fn fetch_sizes(
    session: &mut ImapSession,
    uids: &[i64],
) -> Result<Vec<(i64, u32)>, AppError> {
    if uids.is_empty() {
        return Ok(Vec::new());
    }
    let set = uids
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let stream = session
        .uid_fetch(set, "(UID RFC822.SIZE)")
        .await
        .map_err(imap_err)?;
    let fetches: Vec<Fetch> = stream.try_collect().await.map_err(imap_err)?;
    Ok(fetches
        .iter()
        .filter_map(|f| Some((i64::from(f.uid?), f.size?)))
        .collect())
}

/// Set or clear the `\Seen` flag on one message by UID — the server-side
/// read state. `.SILENT` suppresses the echoed FETCH, but the tagged
/// response still has to be drained for the command to complete.
pub async fn set_seen(session: &mut ImapSession, uid: i64, seen: bool) -> Result<(), AppError> {
    set_seen_many(session, &[uid], seen).await
}

/// The IMAP set notation for a list of UIDs: `"3,1,2"`.
fn uid_set(uids: &[i64]) -> String {
    uids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Flip `\Seen` on a whole set of UIDs in the selected folder.
///
/// why one command and not one per uid: UID STORE takes a set, so marking
/// fifty messages read is a single round trip rather than fifty.
pub async fn set_seen_many(
    session: &mut ImapSession,
    uids: &[i64],
    seen: bool,
) -> Result<(), AppError> {
    if uids.is_empty() {
        return Ok(());
    }
    let query = if seen {
        "+FLAGS.SILENT (\\Seen)"
    } else {
        "-FLAGS.SILENT (\\Seen)"
    };
    let updates = session
        .uid_store(uid_set(uids), query)
        .await
        .map_err(imap_err)?;
    let _: Vec<Fetch> = updates.try_collect().await.map_err(imap_err)?;
    Ok(())
}

/// Move one message by UID to `dest` (e.g. the Trash folder). Prefers the
/// atomic MOVE (RFC 6851); on servers without it, falls back to
/// COPY + mark `\Deleted` + EXPUNGE, expunging only this UID where UIDPLUS
/// allows so other clients' pending deletions are left untouched.
pub async fn move_message(session: &mut ImapSession, uid: i64, dest: &str) -> Result<(), AppError> {
    let uid = uid.to_string();
    // why: read both capabilities up front — Capabilities is owned, so it
    // holds no borrow on the session, unlike the expunge streams below.
    let caps = session.capabilities().await.ok();
    let supports = |name: &str| caps.as_ref().is_some_and(|c| c.has_str(name));

    if supports("MOVE") {
        return session.uid_mv(&uid, dest).await.map_err(imap_err);
    }

    session.uid_copy(&uid, dest).await.map_err(imap_err)?;
    expunge_uid(session, &uid).await
}

/// Permanently remove one message by UID from the selected mailbox — no
/// copy anywhere. This is how a superseded or sent draft version dies.
pub async fn delete_message(session: &mut ImapSession, uid: i64) -> Result<(), AppError> {
    expunge_uid(session, &uid.to_string()).await
}

/// Find the UID of the message carrying `message_id` in the selected
/// mailbox; `None` when the server no longer has it. The highest UID wins
/// if several match — later versions of a draft get later UIDs.
pub async fn find_by_message_id(
    session: &mut ImapSession,
    message_id: &str,
) -> Result<Option<i64>, AppError> {
    let uids = session
        .uid_search(message_id_query(message_id))
        .await
        .map_err(imap_err)?;
    Ok(uids.into_iter().max().map(i64::from))
}

/// The SEARCH query matching one Message-ID header. Quoted so the id stays
/// a single argument; quotes and backslashes are stripped rather than
/// escaped — no real Message-ID contains them, and a broken id must not be
/// able to smuggle extra search terms into the command.
fn message_id_query(message_id: &str) -> String {
    let clean: String = message_id
        .chars()
        .filter(|c| !matches!(c, '"' | '\\' | '\r' | '\n'))
        .collect();
    format!("HEADER Message-ID \"{clean}\"")
}

/// Mark one UID `\Deleted` and expunge it from the selected mailbox.
async fn expunge_uid(session: &mut ImapSession, uid: &str) -> Result<(), AppError> {
    let updates = session
        .uid_store(uid, "+FLAGS.SILENT (\\Deleted)")
        .await
        .map_err(imap_err)?;
    let _: Vec<Fetch> = updates.try_collect().await.map_err(imap_err)?;
    // why: UID EXPUNGE removes only this message; without UIDPLUS the plain
    // EXPUNGE clears the whole \Deleted set (the RFC's documented fallback).
    let caps = session.capabilities().await.ok();
    if caps.as_ref().is_some_and(|c| c.has_str("UIDPLUS")) {
        let stream = session.uid_expunge(uid).await.map_err(imap_err)?;
        let _: Vec<_> = stream.try_collect().await.map_err(imap_err)?;
    } else {
        let stream = session.expunge().await.map_err(imap_err)?;
        let _: Vec<_> = stream.try_collect().await.map_err(imap_err)?;
    }
    Ok(())
}

/// Upload one raw RFC-2822 message into a mailbox with the given IMAP flag
/// set — `(\Seen)` for Sent copies, `(\Draft \Seen)` for drafts.
pub async fn append(
    session: &mut ImapSession,
    mailbox: &str,
    flags: &str,
    message: &[u8],
) -> Result<(), AppError> {
    session
        .append(mailbox, Some(flags), None, message)
        .await
        .map_err(imap_err)
}

/// Fetch one full raw message by UID; `None` when the server has no such UID.
pub async fn fetch_body(session: &mut ImapSession, uid: i64) -> Result<Option<Vec<u8>>, AppError> {
    let stream = session
        .uid_fetch(uid.to_string(), "(UID BODY.PEEK[])")
        .await
        .map_err(imap_err)?;
    let fetches: Vec<Fetch> = stream.try_collect().await.map_err(imap_err)?;
    Ok(fetches
        .iter()
        .find(|f| f.uid.map(i64::from) == Some(uid))
        .and_then(|f| f.body().map(<[u8]>::to_vec)))
}

fn imap_err(err: async_imap::error::Error) -> AppError {
    AppError::Imap(err.to_string())
}

fn raw_header(fetch: &Fetch) -> Option<RawHeader> {
    Some(RawHeader {
        uid: i64::from(fetch.uid?),
        read: fetch.flags().any(|f| matches!(f, Flag::Seen)),
        has_attachments: fetch.bodystructure().is_some_and(structure_has_attachments),
        header: fetch.header()?.to_vec(),
    })
}

/// Whether a BODYSTRUCTURE describes any real attachment — good enough for
/// the list's paperclip before a body exists; parsing the fetched body
/// later (set_body) replaces it with the exact truth.
fn structure_has_attachments(structure: &imap_proto::types::BodyStructure<'_>) -> bool {
    use imap_proto::types::BodyStructure;
    match structure {
        BodyStructure::Multipart { bodies, .. } => bodies.iter().any(structure_has_attachments),
        // A message nested inside the mail (message/rfc822) is an attachment.
        BodyStructure::Message { .. } => true,
        // Text parts are body alternatives unless explicitly detached.
        BodyStructure::Text { common, .. } => {
            disposition_is(common.disposition.as_ref(), "attachment")
        }
        // Non-text leaves (application/pdf, images…): attachments unless
        // explicitly inline; an undecorated image is treated as embedded.
        BodyStructure::Basic { common, .. } => match common.disposition.as_ref() {
            Some(disposition) => disposition.ty.eq_ignore_ascii_case("attachment"),
            None => !common.ty.ty.eq_ignore_ascii_case("image"),
        },
    }
}

fn disposition_is(
    disposition: Option<&imap_proto::types::ContentDisposition<'_>>,
    ty: &str,
) -> bool {
    disposition.is_some_and(|d| d.ty.eq_ignore_ascii_case(ty))
}

/// Sequence range covering the newest `count` messages of a mailbox holding
/// `exists` messages; `None` for an empty mailbox.
pub fn initial_seq_range(exists: u32, count: u32) -> Option<String> {
    if exists == 0 || count == 0 {
        return None;
    }
    let start = exists.saturating_sub(count - 1).max(1);
    Some(format!("{start}:*"))
}

/// Every UID the selected folder holds. Numbers only (UID SEARCH), no
/// headers; the backfill diffs this against the cache and pages through the
/// difference with `older_uid_page`.
pub async fn search_all_uids(session: &mut ImapSession) -> Result<Vec<i64>, AppError> {
    let uids = session.uid_search("ALL").await.map_err(imap_err)?;
    Ok(uids.into_iter().map(i64::from).collect())
}

/// The next backfill page: the `count` largest UIDs, newest first, so the
/// mailbox fills in backwards from where the cache ends.
pub fn older_uid_page(mut uids: Vec<i64>, count: usize) -> Vec<i64> {
    uids.sort_unstable_by(|a, b| b.cmp(a));
    uids.truncate(count);
    uids
}

/// Drop everything at or below the last cached UID.
///
/// why: `UID FETCH last+1:*` always returns at least the mailbox's newest
/// message even when nothing is new (RFC 3501 quirk) — without this filter
/// every incremental sync would re-upsert the newest message.
pub fn new_uids_only(headers: Vec<RawHeader>, last_uid: i64) -> Vec<RawHeader> {
    headers.into_iter().filter(|h| h.uid > last_uid).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xoauth2_sends_user_and_bearer_token_once() {
        use async_imap::Authenticator;
        let mut auth = XOAuth2::new("jan@gmail.com", "ya29.token");
        assert_eq!(
            auth.process(b""),
            "user=jan@gmail.com\x01auth=Bearer ya29.token\x01\x01"
        );
        // A rejected token comes back as a second challenge (a JSON error);
        // the protocol wants an empty reply to it, never the token again.
        assert_eq!(auth.process(br#"{"status":"401"}"#), "");
    }

    #[tokio::test]
    async fn a_token_login_answers_the_servers_prompt_after_its_greeting() {
        use base64::Engine;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let (client_end, server_end) = tokio::io::duplex(4096);
        // A server talking like Gmail: greeting first, then an empty "+"
        // prompt for the SASL response, then the tagged OK.
        let server = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server_end);
            let mut lines = BufReader::new(read).lines();
            write.write_all(b"* OK Gimap ready\r\n").await.unwrap();
            let command = lines.next_line().await.unwrap().unwrap();
            let (tag, verb) = command.split_once(' ').unwrap();
            assert_eq!(verb, "AUTHENTICATE XOAUTH2");
            write.write_all(b"+ \r\n").await.unwrap();
            let response = lines.next_line().await.unwrap().unwrap();
            let reply = format!("{tag} OK jan@gmail.com authenticated (Success)\r\n");
            write.write_all(reply.as_bytes()).await.unwrap();
            response
        });

        let token = Credential::AccessToken("ya29.token".to_string());
        let login = log_in(client_end, "jan@gmail.com", &token);
        tokio::time::timeout(Duration::from_secs(5), login)
            .await
            .expect("the login hung")
            .expect("the login failed");
        let response = server.await.unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(response)
            .unwrap();
        assert_eq!(
            decoded,
            b"user=jan@gmail.com\x01auth=Bearer ya29.token\x01\x01"
        );
    }

    #[test]
    fn a_uid_set_is_comma_separated() {
        assert_eq!(uid_set(&[]), "");
        assert_eq!(uid_set(&[42]), "42");
        // Order is the caller's; the server does not care.
        assert_eq!(uid_set(&[3, 1, 2]), "3,1,2");
    }

    #[test]
    fn an_empty_folder_has_nothing_to_sweep() {
        assert_eq!(sweep_range(0, None), None);
        assert_eq!(sweep_range(0, Some(1000)), None);
    }

    #[test]
    fn no_window_sweeps_the_whole_folder() {
        assert_eq!(sweep_range(55_662, None).as_deref(), Some("1:*"));
        assert_eq!(sweep_range(1, None).as_deref(), Some("1:*"));
    }

    #[test]
    fn a_window_takes_the_newest_messages() {
        assert_eq!(sweep_range(55_662, Some(1_000)).as_deref(), Some("54663:*"));
        assert_eq!(sweep_range(10, Some(3)).as_deref(), Some("8:*"));
    }

    #[test]
    fn a_window_covering_the_folder_sweeps_all_of_it() {
        // Off-by-one guard: a window equal to the count must stay at 1:*,
        // never 2:*.
        assert_eq!(sweep_range(10, Some(10)).as_deref(), Some("1:*"));
        assert_eq!(sweep_range(10, Some(11)).as_deref(), Some("1:*"));
        assert_eq!(sweep_range(10, Some(0)).as_deref(), Some("1:*"));
    }

    fn raw(uid: i64) -> RawHeader {
        RawHeader {
            uid,
            read: false,
            has_attachments: false,
            header: Vec::new(),
        }
    }

    use imap_proto::types::{
        BodyContentCommon, BodyContentSinglePart, BodyStructure, ContentDisposition,
        ContentEncoding, ContentType,
    };
    use std::borrow::Cow;

    fn part(
        ty: &'static str,
        subtype: &'static str,
        disposition: Option<&'static str>,
    ) -> BodyStructure<'static> {
        let common = BodyContentCommon {
            ty: ContentType {
                ty: Cow::Borrowed(ty),
                subtype: Cow::Borrowed(subtype),
                params: None,
            },
            disposition: disposition.map(|d| ContentDisposition {
                ty: Cow::Borrowed(d),
                params: None,
            }),
            language: None,
            location: None,
        };
        let other = BodyContentSinglePart {
            id: None,
            md5: None,
            description: None,
            transfer_encoding: ContentEncoding::Base64,
            octets: 0,
        };
        if ty.eq_ignore_ascii_case("text") {
            BodyStructure::Text {
                common,
                other,
                lines: 1,
                extension: None,
            }
        } else {
            BodyStructure::Basic {
                common,
                other,
                extension: None,
            }
        }
    }

    fn multipart(bodies: Vec<BodyStructure<'static>>) -> BodyStructure<'static> {
        BodyStructure::Multipart {
            common: BodyContentCommon {
                ty: ContentType {
                    ty: Cow::Borrowed("multipart"),
                    subtype: Cow::Borrowed("mixed"),
                    params: None,
                },
                disposition: None,
                language: None,
                location: None,
            },
            bodies,
            extension: None,
        }
    }

    #[test]
    fn plain_and_alternative_bodies_have_no_attachments() {
        assert!(!structure_has_attachments(&part("text", "plain", None)));
        assert!(!structure_has_attachments(&multipart(vec![
            part("text", "plain", None),
            part("text", "html", None),
        ])));
    }

    #[test]
    fn detached_files_count_as_attachments() {
        // The classic: text body + a PDF with an attachment disposition.
        assert!(structure_has_attachments(&multipart(vec![
            part("text", "plain", None),
            part("application", "pdf", Some("attachment")),
        ])));
        // A csv exported as text also announces itself via disposition.
        assert!(structure_has_attachments(&multipart(vec![
            part("text", "plain", None),
            part("text", "csv", Some("attachment")),
        ])));
        // A PDF without any disposition is still a file, not a body.
        assert!(structure_has_attachments(&multipart(vec![
            part("text", "plain", None),
            part("application", "pdf", None),
        ])));
    }

    #[test]
    fn embedded_images_do_not_count() {
        // multipart/related html mail with cid images: inline disposition…
        assert!(!structure_has_attachments(&multipart(vec![
            part("text", "html", None),
            part("image", "png", Some("inline")),
        ])));
        // …or none at all — undecorated images are treated as embedded.
        assert!(!structure_has_attachments(&multipart(vec![
            part("text", "html", None),
            part("image", "jpeg", None),
        ])));
        // An image the sender explicitly detached still counts.
        assert!(structure_has_attachments(&multipart(vec![
            part("text", "plain", None),
            part("image", "jpeg", Some("attachment")),
        ])));
    }

    #[test]
    fn initial_range_covers_the_newest_n() {
        assert_eq!(initial_seq_range(100, 50), Some("51:*".to_string()));
        assert_eq!(initial_seq_range(50, 50), Some("1:*".to_string()));
    }

    #[test]
    fn initial_range_clamps_small_mailboxes() {
        assert_eq!(initial_seq_range(10, 50), Some("1:*".to_string()));
        assert_eq!(initial_seq_range(1, 50), Some("1:*".to_string()));
    }

    #[test]
    fn initial_range_is_none_for_empty_mailbox() {
        assert_eq!(initial_seq_range(0, 50), None);
        assert_eq!(initial_seq_range(10, 0), None);
    }

    #[test]
    fn older_uid_page_picks_the_newest_n() {
        // SEARCH results arrive unordered; the page is the N largest, newest
        // first, so backfill walks the mailbox backwards in date-ish order.
        assert_eq!(older_uid_page(vec![4, 40, 2, 30, 7], 3), vec![40, 30, 7]);
    }

    #[test]
    fn older_uid_page_returns_everything_when_fewer_than_n() {
        assert_eq!(older_uid_page(vec![2, 9], 500), vec![9, 2]);
        assert_eq!(older_uid_page(Vec::new(), 500), Vec::<i64>::new());
    }

    #[test]
    fn new_uids_only_drops_already_cached() {
        let filtered = new_uids_only(vec![raw(9), raw(10), raw(11), raw(12)], 10);

        let uids: Vec<i64> = filtered.iter().map(|h| h.uid).collect();
        assert_eq!(uids, vec![11, 12]);
    }

    #[test]
    fn message_id_query_quotes_the_id() {
        assert_eq!(
            message_id_query("flit-draft-1-0@flit.local"),
            "HEADER Message-ID \"flit-draft-1-0@flit.local\""
        );
    }

    #[test]
    fn message_id_query_strips_characters_that_break_out_of_the_quotes() {
        assert_eq!(
            message_id_query("evil\"\r\nid\\@x"),
            "HEADER Message-ID \"evilid@x\""
        );
    }

    #[test]
    fn discovered_skips_unselectable_folders() {
        assert_eq!(discovered("dovecot", &[NameAttribute::NoSelect]), None);
    }

    #[test]
    fn discovered_maps_special_use_attributes_to_roles() {
        let junk = discovered("Spam", &[NameAttribute::Junk]).unwrap();
        assert_eq!(junk.role.as_deref(), Some("junk"));

        let sent = discovered("Odoslané", &[NameAttribute::Sent]).unwrap();
        assert_eq!(sent.role.as_deref(), Some("sent"));
        assert_eq!(sent.name, "Odoslané");

        // Gmail's localized "All Mail" carries \All, not \Archive.
        let all = discovered("[Gmail]/Všetky správy", &[NameAttribute::All]).unwrap();
        assert_eq!(all.role.as_deref(), Some("all"));
    }

    #[test]
    fn discovered_falls_back_to_well_known_names() {
        // Servers without SPECIAL-USE only send generic attributes.
        let trash = discovered("Trash", &[NameAttribute::Unmarked]).unwrap();
        assert_eq!(trash.role.as_deref(), Some("trash"));

        let spam = discovered("spam", &[]).unwrap();
        assert_eq!(spam.role.as_deref(), Some("junk"));

        assert_eq!(
            discovered("INBOX", &[]).unwrap().role.as_deref(),
            Some("inbox")
        );
        assert_eq!(discovered("Projects", &[]).unwrap().role, None);
    }
}
