//! No request the person did not ask for (brief 0032): what turns Chromium's background services off.
//!
//! CEF 154 runs Chrome's browser process, with its services: brief 0031's net log of a fresh profile found requests to
//! `accounts.google.com` (the signed-in account check), `update.googleapis.com` (the component updater),
//! `www.google.com` (a preconnect to the search engine and the AI Mode eligibility check, `/async/folae`),
//! `clients2.google.com` (network time) and `dns.google` (the DNS-over-HTTPS upgrade probe). Each is turned off here by
//! a switch ([`SWITCHES`]), a feature flag ([`DISABLED_FEATURES`]), a preference written into the profile before
//! CEF starts ([`seed_profile`]) or a Chrome policy ([`policies`], read through CEF's `chrome_policy_id`);
//! `browsers/chromium/README.md` lists which silences what, and the engine's net-log test proves that nothing leaves
//! on `about:blank`.

use std::path::Path;

use serde_json::{Value, json};

/// Switches (without `--`) for the browser process, beside the rendering ones.
pub const SWITCHES: &[&str] = &[
    // Chrome's background requests: variations, the safe-browsing database, the promotion fetches.
    "disable-background-networking",
    // The component updater (update.googleapis.com) for every component registered at startup.
    "disable-component-update",
    "disable-default-apps",
    "disable-sync",
    "no-first-run",
    "no-default-browser-check",
    "no-pings",
    "no-service-autorun",
    "disable-breakpad",
    "disable-client-side-phishing-detection",
    "disable-domain-reliability",
    "disable-field-trial-config",
    "disable-search-engine-choice-screen",
    "metrics-recording-only",
    // No desktop keyring prompt for a profile that keeps no passwords of the user's.
    "password-store=basic",
    // Chrome's own Google account check (accounts.google.com/ListAccounts) has no off switch: the account service
    // lists the cookie jar's accounts whenever anything asks, sign-in allowed or not. Its url is pointed at one
    // that is not on the network (an empty data url, which is not a valid ListAccounts url), so each attempt fails
    // inside the engine. Pages that sign in with Google are unaffected: only this url of Chrome's account service
    // changes.
    r#"gaia-config-contents={"urls":{"list_accounts_url":{"url":"data:,"}}}"#,
    // `--disable-component-update` leaves the component updater's on-demand checks on (the on-device model's
    // manifest asks update.googleapis.com at startup): its server is pointed at a url that is not on the network.
    "component-updater=url-source=data:,",
];

/// `--disable-features=`, each with what it silences.
pub const DISABLED_FEATURES: &[(&str, &str)] = &[
    ("NetworkTimeServiceQuerying", "clients2.google.com/time (network time)"),
    ("OptimizationHints", "the optimization guide's hint fetches"),
    ("MediaRouter", "Cast discovery"),
    ("DialMediaRouteProvider", "DIAL discovery on the local network"),
    ("Translate", "the translate script and language model downloads"),
    ("CertificateTransparencyComponentUpdater", "the CT log list component"),
    ("LensOverlay", "Lens"),
    ("AutofillServerCommunication", "autofill's field-type queries"),
    ("AimEnabled", "AI Mode (www.google.com/async/folae)"),
    ("AimServerEligibilityEnabled", "AI Mode's eligibility request (www.google.com/async/folae)"),
    ("AimServerRequestOnStartupEnabled", "the same request at profile load"),
    ("PreconnectToSearch", "the preconnect to the default search engine (www.google.com)"),
    ("DnsOverHttpsUpgrade", "the DNS-over-HTTPS upgrade probes (dns.google)"),
];

/// The `--disable-features=` switch.
pub fn disable_features_switch() -> String {
    let names: Vec<&str> = DISABLED_FEATURES.iter().map(|(n, _)| *n).collect();
    format!("disable-features={}", names.join(","))
}

/// `a,b` and `b,c` to `a,b,c`: a feature list switch CEF set already, with ours added.
pub fn merge_feature_lists(old: &str, ours: &str) -> String {
    let mut names: Vec<&str> = Vec::new();
    for n in old.split(',').chain(ours.split(',')) {
        let n = n.trim();
        if !n.is_empty() && !names.contains(&n) {
            names.push(n);
        }
    }
    names.join(",")
}

/// Preferences of the profile (`<profile>/Default/Preferences`), each with what it silences.
pub fn profile_preferences() -> Value {
    json!({
        // No sign-in to the browser (account consistency is computed at startup from `allowed_on_next_startup`).
        "signin": {"allowed": false, "allowed_on_next_startup": false},
        // No preconnects or prefetches (www.google.com at startup, links the page did not ask for).
        "net": {"network_prediction_options": 2},
        // Safe Browsing's lookups and database (safebrowsing.googleapis.com).
        "safebrowsing": {"enabled": false, "enhanced": false},
        // Search suggestions, the spelling service, translate, alternate error pages: requests per keystroke or page.
        "search": {"suggest_enabled": false},
        "spellcheck": {"use_spelling_service": false},
        "browser": {"enable_spellchecking": false},
        "translate": {"enabled": false},
        "alternate_error_pages": {"enabled": false},
        // Password and payment services of the user's Google account.
        "credentials_enable_service": false,
        "autofill": {"profile_enabled": false, "credit_card_enabled": false},
    })
}

/// Preferences of the browser (`<profile>/Local State`).
pub fn local_state() -> Value {
    json!({
        // DNS through the system resolver only: no DNS-over-HTTPS upgrade, no probes of dns.google.
        "dns_over_https": {"mode": "off"},
        // No on-device model (its manifest is a component the updater asks update.googleapis.com for): the
        // local-state twin of the GenAILocalFoundationalModelSettings policy, 1 = do not download.
        "optimization_guide": {"gen_ai_local_foundational_model_settings": 1},
    })
}

/// Chrome policies, written into `<profile>/Policies/managed/eludite.json` and read through CEF's `chrome_policy_id`
/// (on Linux, a folder of policy files): what preferences cannot reach because Chrome reads them too early, or
/// lets the person override.
pub fn policies() -> Value {
    json!({
        // No sign-in to the browser: no account checks.
        "BrowserSignin": 0,
        "SyncDisabled": true,
        // The component updater (update.googleapis.com).
        "ComponentUpdatesEnabled": false,
        "SafeBrowsingProtectionLevel": 0,
        "NetworkPredictionOptions": 2,
        "DnsOverHttpsMode": "off",
        "SearchSuggestEnabled": false,
        "MetricsReportingEnabled": false,
        "UrlKeyedAnonymizedDataCollectionEnabled": false,
        "TranslateEnabled": false,
        "SpellCheckServiceEnabled": false,
        "AlternateErrorPagesEnabled": false,
        "BackgroundModeEnabled": false,
        "PasswordManagerEnabled": false,
        "AutofillAddressEnabled": false,
        "AutofillCreditCardEnabled": false,
        "DefaultBrowserSettingEnabled": false,
        "PromotionalTabsEnabled": false,
        "ShoppingListEnabled": false,
        // No on-device model download (its manifest is a component: update.googleapis.com).
        "GenAILocalFoundationalModelSettings": 1,
    })
}

/// The policy folder CEF's `chrome_policy_id` names, inside the profile.
pub fn policy_dir(profile: &Path) -> std::path::PathBuf {
    profile.join("Policies")
}

/// Merge `patch` into `base`, objects member by member; every other value of `patch` replaces `base`'s.
pub fn merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                merge(b.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (b, p) => *b = p.clone(),
    }
}

fn merge_file(path: &Path, patch: &Value) -> std::io::Result<()> {
    let mut v = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    merge(&mut v, patch);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("eludite-tmp");
    std::fs::write(&tmp, serde_json::to_vec(&v).unwrap_or_default())?;
    std::fs::rename(tmp, path)
}

/// Write the preferences into the profile before CEF reads it (the profile is CEF's root cache path; Chrome keeps
/// the browser's preferences in `Local State` and the profile's in `Default/Preferences`). Keys already there are
/// kept unless these set them.
pub fn seed_profile(profile: &Path) -> std::io::Result<()> {
    let managed = policy_dir(profile).join("managed");
    std::fs::create_dir_all(&managed)?;
    std::fs::write(
        managed.join("eludite.json"),
        serde_json::to_vec_pretty(&policies()).unwrap_or_default(),
    )?;
    merge_file(&profile.join("Local State"), &local_state())?;
    merge_file(
        &profile.join("Default").join("Preferences"),
        &profile_preferences(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_is_seeded_and_keeps_its_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = dir.path().join("Default").join("Preferences");
        std::fs::create_dir_all(prefs.parent().unwrap()).unwrap();
        std::fs::write(&prefs, r#"{"signin": {"allowed": true, "other": 1}, "kept": "yes"}"#).unwrap();
        seed_profile(dir.path()).unwrap();
        seed_profile(dir.path()).unwrap();
        let p: Value = serde_json::from_slice(&std::fs::read(&prefs).unwrap()).unwrap();
        assert_eq!(p["signin"]["allowed"], false);
        assert_eq!(p["signin"]["allowed_on_next_startup"], false);
        assert_eq!(p["signin"]["other"], 1);
        assert_eq!(p["kept"], "yes");
        assert_eq!(p["net"]["network_prediction_options"], 2);
        assert_eq!(p["safebrowsing"]["enabled"], false);
        let l: Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("Local State")).unwrap()).unwrap();
        assert_eq!(l["dns_over_https"]["mode"], "off");
        // A file that is not JSON is replaced.
        std::fs::write(&prefs, "garbage").unwrap();
        seed_profile(dir.path()).unwrap();
        let p: Value = serde_json::from_slice(&std::fs::read(&prefs).unwrap()).unwrap();
        assert_eq!(p["signin"]["allowed"], false);
    }

    #[test]
    fn the_account_check_and_the_updater_point_off_the_network() {
        let gaia = SWITCHES
            .iter()
            .find_map(|s| s.strip_prefix("gaia-config-contents="))
            .unwrap();
        let v: Value = serde_json::from_str(gaia).unwrap();
        assert_eq!(v["urls"]["list_accounts_url"]["url"], "data:,");
        assert!(SWITCHES.contains(&"component-updater=url-source=data:,"));
        assert!(SWITCHES.contains(&"disable-component-update"));
    }

    #[test]
    fn feature_lists_merge_without_repeats() {
        assert_eq!(merge_feature_lists("A,B", "B,C"), "A,B,C");
        assert_eq!(merge_feature_lists("", "C"), "C");
    }

    #[test]
    fn every_feature_is_named_once_with_what_it_silences() {
        let s = disable_features_switch();
        assert!(s.starts_with("disable-features=NetworkTimeServiceQuerying,"));
        let mut names: Vec<_> = DISABLED_FEATURES.iter().map(|(n, _)| *n).collect();
        assert!(DISABLED_FEATURES.iter().all(|(n, why)| !n.contains(',') && !why.is_empty()));
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), DISABLED_FEATURES.len());
    }
}
