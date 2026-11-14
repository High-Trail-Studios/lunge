//! Suggesting a profile name for an account role:
//! `<session>-<account>-<role>`, lowercase, spaces as `_`, common words
//! shortened (production -> prod, AdministratorAccess -> admin, ...). Only a
//! starting point: the user confirms or edits it before anything is created.

/// Longer words in account names and their usual short forms.
const ACCOUNT_WORDS: [(&str, &str); 8] = [
    ("production", "prod"),
    ("development", "dev"),
    ("staging", "stg"),
    ("testing", "test"),
    ("sandbox", "sbx"),
    ("management", "mgmt"),
    ("security", "sec"),
    ("shared_services", "shared"),
];

/// AWS-managed permission set names and their short forms; anything else is
/// split from CamelCase and has a trailing "Access" dropped.
const ROLE_NAMES: [(&str, &str); 6] = [
    ("administratoraccess", "admin"),
    ("awsadministratoraccess", "admin"),
    ("poweruseraccess", "poweruser"),
    ("awspoweruseraccess", "poweruser"),
    ("readonlyaccess", "readonly"),
    ("viewonlyaccess", "viewonly"),
];

/// Longest suggestion before the account part is trimmed.
const MAX_LEN: usize = 60;

/// Suggests a profile name not rejected by `is_taken`, appending `-2`, `-3`,
/// ... if needed. Always passes [`crate::rename::validate_name`].
pub fn suggest_profile_name(
    session: &str,
    account_name: &str,
    role_name: &str,
    is_taken: impl Fn(&str) -> bool,
) -> String {
    let session = slug(session);
    let role = role_part(role_name);
    let mut account = account_part(account_name);
    let fixed = session.len() + role.len() + 2;
    if fixed + account.len() > MAX_LEN {
        account.truncate(MAX_LEN.saturating_sub(fixed).max(1));
        account = account.trim_end_matches(['_', '-', '.']).to_string();
    }
    let parts: Vec<&str> = [session.as_str(), account.as_str(), role.as_str()]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    let base = if parts.is_empty() {
        "profile".to_string()
    } else {
        parts.join("-")
    };
    if !is_taken(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| !is_taken(name))
        .expect("some suffix is free")
}

fn account_part(account_name: &str) -> String {
    let mut slugged = format!("_{}_", slug(account_name));
    for (long, short) in ACCOUNT_WORDS {
        slugged = slugged.replace(&format!("_{long}_"), &format!("_{short}_"));
    }
    slugged.trim_matches('_').to_string()
}

fn role_part(role_name: &str) -> String {
    let lower = role_name.to_lowercase();
    if let Some((_, short)) = ROLE_NAMES.iter().find(|(long, _)| *long == lower) {
        return short.to_string();
    }
    let split = split_camel_case(role_name);
    let slugged = slug(&split);
    slugged
        .strip_suffix("_access")
        .unwrap_or(&slugged)
        .to_string()
}

/// "DataScientistAccess" -> "Data Scientist Access"; acronyms stay whole
/// ("AWSBilling" -> "AWS Billing").
fn split_camel_case(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && c.is_uppercase() {
            let prev = chars[i - 1];
            let next_is_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || (prev.is_uppercase() && next_is_lower) {
                out.push(' ');
            }
        }
        out.push(c);
    }
    out
}

/// Lowercase; whitespace and anything outside `[a-z0-9._-]` becomes `_`;
/// runs of `_` collapse; leading/trailing separators go.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().to_lowercase().chars() {
        let c = if c.is_ascii_alphanumeric() || matches!(c, '-' | '.') {
            c
        } else {
            '_'
        };
        if c == '_' && out.ends_with('_') {
            continue;
        }
        out.push(c);
    }
    out.trim_matches(['_', '-', '.']).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rename::validate_name;

    fn suggest(session: &str, account: &str, role: &str) -> String {
        suggest_profile_name(session, account, role, |_| false)
    }

    #[test]
    fn follows_the_session_account_role_pattern() {
        assert_eq!(
            suggest("hjedi", "Jedi Production", "AdministratorAccess"),
            "hjedi-jedi_prod-admin"
        );
        assert_eq!(
            suggest("Work", "Shared Services", "ReadOnlyAccess"),
            "work-shared-readonly"
        );
        assert_eq!(
            suggest("work", "Data Lake (Dev)", "DataScientistAccess"),
            "work-data_lake_dev-data_scientist"
        );
        assert_eq!(
            suggest("work", "staging", "AWSBillingAccess"),
            "work-stg-aws_billing"
        );
        assert_eq!(
            suggest("work", "acme-sandbox", "PowerUserAccess"),
            "work-acme-sandbox-poweruser"
        );
    }

    #[test]
    fn avoids_taken_names() {
        let taken = ["work-prod-admin", "work-prod-admin-2"];
        let name = suggest_profile_name("work", "Production", "AdministratorAccess", |n| {
            taken.contains(&n)
        });
        assert_eq!(name, "work-prod-admin-3");
    }

    #[test]
    fn long_and_odd_names_still_validate() {
        for (session, account, role) in [
            (
                "work",
                "A Very Long Account Name For The Central Platform Engineering Team Production",
                "AdministratorAccess",
            ),
            ("", "", ""),
            ("  ", "Ünïcödé Prød!!", "Rôle"),
            ("work", "123456789012", "x"),
        ] {
            let name = suggest(session, account, role);
            assert!(validate_name(&name).is_ok(), "{name:?}");
            assert!(name.len() <= MAX_LEN, "{name:?} is {} long", name.len());
        }
    }
}
