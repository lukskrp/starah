//! Minimal polkit client: may the D-Bus caller (a human) perform a Starah action?

use std::collections::HashMap;

use zbus::zvariant::Value;

#[zbus::proxy(
    interface = "org.freedesktop.PolicyKit1.Authority",
    default_service = "org.freedesktop.PolicyKit1",
    default_path = "/org/freedesktop/PolicyKit1/Authority"
)]
trait Authority {
    fn check_authorization(
        &self,
        subject: &(&str, HashMap<&str, Value<'_>>),
        action_id: &str,
        details: HashMap<&str, &str>,
        flags: u32,
        cancellation_id: &str,
    ) -> zbus::Result<(bool, bool, HashMap<String, String>)>;
}

const ALLOW_USER_INTERACTION: u32 = 1;

/// `Ok(true)` if `sender` (a unique bus name) is authorized for `action`; polkit may
/// ask the human for a password through their session's authentication agent.
pub async fn check(conn: &zbus::Connection, sender: &str, action: &str) -> zbus::Result<bool> {
    let authority = AuthorityProxy::new(conn).await?;
    let mut subject = HashMap::new();
    subject.insert("name", Value::from(sender));
    let (authorized, _, _) = authority
        .check_authorization(
            &("system-bus-name", subject),
            action,
            HashMap::new(),
            ALLOW_USER_INTERACTION,
            "",
        )
        .await?;
    Ok(authorized)
}
