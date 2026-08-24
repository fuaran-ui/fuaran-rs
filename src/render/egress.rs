//! Destination policy — typed egress allowlists (`WIRE_FORMAT.md` §14.1).
//!
//! [`super::sanitize`] answers *is this URL safe to have*. It does not answer
//! *is this destination one the composition declared*, and only the second
//! question closes exfiltration: `https://collector.example/?s=<bound state>`
//! passes every rule in the scheme floor — allowlisted scheme, well-formed
//! host, no script anywhere in it — and in an image `src` the browser contacts
//! it with **no user act at all**, because rendering *is* the request.
//!
//! So the floor gains a second, orthogonal gate: a scheme allowlist says what a
//! URL may BE, and an origin allowlist says where it may GO. Both are positive
//! lists; neither substitutes for the other, and this one runs after the other
//! because there is no point asking where an unsafe URL points.
//!
//! Two shapes are deliberate and worth stating, because both look like
//! omissions:
//!
//! - A rule names a **host**, never a scheme and never a path. Scheme is
//!   already reduced to the allowlisted set by the floor, and every "scheme
//!   wildcard" spelling anyone reaches for (`*://`, `http*://`, `https?://`)
//!   parses differently on different hosts — which makes the wildcard itself
//!   the vulnerability. Path scoping is likewise refused: a path is not a
//!   security boundary, and a policy that appears to bound one invites reliance
//!   on a bound it does not have.
//! - The policy is **host-constructed and never carried on the wire**. A policy
//!   an emission can supply is a policy a hostile emission can widen, which is
//!   not a policy. There is deliberately no decoder on this seam.

use super::sanitize::{extract_scheme, sanitize_url};

/// The classes of destination a rule can be scoped to. Closed by construction:
/// a policy can say something only about a class this enum can name, and a
/// `match` over it is exhaustive at build time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EgressClass {
    /// A rendered `href` the reader must ACT on — a link, an autolink.
    Hyperlink,
    /// A rendered `src` the browser fetches with NO user act: an image, a
    /// stylesheet, a media element. THE exfiltration class — a destination here
    /// is contacted merely by rendering the tree, which is why it is scoped
    /// separately from [`EgressClass::Hyperlink`] rather than folded in with it.
    Media,
    /// A navigation the tree asks for.
    Route,
    /// A file download the tree asks for.
    Download,
    /// A file READ the tree asks for. It carries no URL of its own, but it is
    /// scoped here so a policy can speak about it in the same vocabulary.
    FileRead,
}

impl EgressClass {
    /// Every class, in wire order. Used by [`EgressPolicy::allow_origin`] when a
    /// rule is declared without a class scope (which means "every class").
    pub const ALL: [EgressClass; 5] = [
        EgressClass::Hyperlink,
        EgressClass::Media,
        EgressClass::Route,
        EgressClass::Download,
        EgressClass::FileRead,
    ];

    /// The stable lowercase wire spelling — what a refusal marker records.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            EgressClass::Hyperlink => "hyperlink",
            EgressClass::Media => "media",
            EgressClass::Route => "route",
            EgressClass::Download => "download",
            EgressClass::FileRead => "fileRead",
        }
    }

    /// Parse a wire spelling. Case-insensitive on the caller's behalf; an
    /// unknown name is `None` rather than a silently-ignored rule, because a
    /// policy that quietly drops a class it did not understand is broader than
    /// the one its author wrote.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let k = s.trim().to_lowercase();
        EgressClass::ALL
            .into_iter()
            .find(|c| c.name().to_lowercase() == k)
    }
}

/// One allowed destination. Hosts only — no scheme, no port, no path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressOrigin {
    /// Exactly this host. `example.com` matches `example.com` and nothing else
    /// — not `a.example.com`, not `notexample.com`.
    ExactHost(String),
    /// This host and any subdomain of it. `example.com` matches `example.com`
    /// and `a.b.example.com`; it never matches `notexample.com`, because the
    /// match requires a **label boundary**. A suffix, not a substring, and not
    /// a wildcard.
    HostSuffix(String),
}

impl EgressOrigin {
    /// Does this origin match an already-normalised host?
    fn matches(&self, host: &str) -> bool {
        match self {
            EgressOrigin::ExactHost(h) => {
                let h = normalize_host(h);
                !h.is_empty() && h == host
            }
            EgressOrigin::HostSuffix(s) => {
                let s = normalize_host(s);
                !s.is_empty() && (host == s || host.ends_with(&format!(".{s}")))
            }
        }
    }
}

/// One rule: an origin, and the classes it is declared FOR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressRule {
    pub origin: EgressOrigin,
    /// The classes this origin is allowed for. An EMPTY list allows no class —
    /// a rule that names nothing permits nothing, which is the only reading
    /// consistent with a positive list. Use [`EgressClass::ALL`] to mean "every
    /// class".
    pub classes: Vec<EgressClass>,
}

/// A typed egress allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressPolicy {
    pub rules: Vec<EgressRule>,
    /// When `true`, EVERY network origin is permitted and `rules` is not
    /// consulted at all.
    ///
    /// This is the escape hatch, and it is a FIELD rather than the absence of
    /// rules on purpose: an empty allowlist must read as "nothing is declared",
    /// never as "everything is fine". Those are opposite postures, and the
    /// empty list is what a half-built policy looks like — so conflating them
    /// would make forgetting to declare anything indistinguishable from
    /// deciding not to.
    pub allow_any_origin: bool,
    /// Whether SAME-ORIGIN destinations (a relative path, a fragment, an empty
    /// URL) are permitted. `true` in both shipped policies: a tree pointing at
    /// its own host has not left, and denying it would make ordinary in-app
    /// links unrenderable.
    pub allow_local: bool,
    /// Whether destinations with no network host (`mailto:`, `tel:`) are
    /// permitted. `false` by default: `mailto:` IS an egress channel — a body
    /// parameter carries arbitrary text off the machine — and it has no host
    /// for a rule to name, so it cannot be allowlisted, only permitted
    /// wholesale.
    pub allow_non_network: bool,
}

impl EgressPolicy {
    /// Declare an origin for a set of classes. An EMPTY class list is taken as
    /// **every** class — the ergonomic reading of "allow this origin", distinct
    /// from an [`EgressRule`] whose `classes` is empty, which permits nothing.
    /// The two readings are deliberately split across the constructor and the
    /// record: the record is data and says exactly what it lists; the helper is
    /// a convenience and says what a caller writing one line means.
    #[must_use]
    pub fn allow_origin(mut self, origin: EgressOrigin, classes: &[EgressClass]) -> Self {
        let classes = if classes.is_empty() {
            EgressClass::ALL.to_vec()
        } else {
            classes.to_vec()
        };
        self.rules.push(EgressRule { origin, classes });
        self
    }

    /// Is this host declared for this class by this policy?
    #[must_use]
    pub fn is_declared_origin(&self, class: EgressClass, host: &str) -> bool {
        let host = normalize_host(host);
        !host.is_empty()
            && (self.allow_any_origin
                || self
                    .rules
                    .iter()
                    .any(|r| r.classes.contains(&class) && r.origin.matches(&host)))
    }

    /// Whether this policy permits anything beyond its own origin — the cheap
    /// answer to the question a manifest reader asks first.
    #[must_use]
    pub fn has_non_local_egress(&self) -> bool {
        self.allow_any_origin
            || self.allow_non_network
            || self.rules.iter().any(|r| !r.classes.is_empty())
    }
}

/// Deny every destination that leaves the origin.
///
/// THE DEFAULT FOR A DECODED (WIRE) TREE. An emission cannot declare its own
/// egress, so absent a host's declaration it gets none.
#[must_use]
pub const fn deny_non_local_egress() -> EgressPolicy {
    EgressPolicy {
        rules: Vec::new(),
        allow_any_origin: false,
        allow_local: true,
        allow_non_network: false,
    }
}

/// Permit every destination.
///
/// The posture for a HAND-AUTHORED tree, where the author is the trust
/// boundary. Named rather than default so reaching it is a deliberate,
/// greppable act.
#[must_use]
pub const fn permissive_egress() -> EgressPolicy {
    EgressPolicy {
        rules: Vec::new(),
        allow_any_origin: true,
        allow_local: true,
        allow_non_network: true,
    }
}

/// What a URL resolves to, once the scheme floor has accepted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Destination {
    /// Same-origin: a relative path, a fragment, an empty URL.
    Local,
    /// An absolute network destination at this host — lowercased, with
    /// userinfo, port and any trailing root dot removed.
    Remote(String),
    /// A scheme with no network host for a rule to name (`mailto:`, `tel:`).
    NonNetwork(String),
    /// The scheme floor rejected the URL, or it declares a network scheme with
    /// no extractable host.
    Rejected,
}

/// Why a destination was refused, or that it was not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressVerdict {
    /// Accepted. Carries the NORMALISED URL to emit — the same string
    /// [`sanitize_url`] would have returned, so an accepting call site needs no
    /// second pass.
    Allowed(String),
    /// The scheme floor rejected it before policy was ever consulted.
    UnsafeUrl,
    /// A network destination whose host this policy does not declare for this
    /// class. Carries the HOST ONLY — never the path or query, which is exactly
    /// where an exfiltrated payload would be sitting.
    UndeclaredOrigin { host: String, class: EgressClass },
    /// A same-origin destination under a policy that denies local egress.
    LocalDenied { class: EgressClass },
    /// A hostless scheme under a policy that denies non-network egress.
    NonNetworkDenied { scheme: String, class: EgressClass },
}

/// Network schemes — the ones that reach a host a rule can name. A scheme the
/// floor allows but that is absent here (`mailto`, `tel`) is
/// [`Destination::NonNetwork`].
const NETWORK_SCHEMES: &[&str] = &["http", "https", "ftp", "sftp"];

/// Lowercase, trim, and drop a single trailing root dot (`example.com.` and
/// `example.com` are the same host to a resolver, so they must be the same host
/// to a policy — otherwise the dotted spelling walks straight past an exact
/// rule).
fn normalize_host(h: &str) -> String {
    let t = h.trim().to_lowercase();
    match t.strip_suffix('.') {
        Some(s) => s.to_string(),
        None => t,
    }
}

/// Extract the host from an absolute URL's authority, WHATWG-style: `\` counts
/// as `/` when locating the authority, userinfo before the **last** `@` is
/// discarded, a port is dropped, and an IPv6 literal keeps its brackets.
///
/// The last-`@` rule is load-bearing rather than fussy:
/// `https://good.example@evil.example/x` is a request to `evil.example`, and a
/// naive first-`@` split reads it as the opposite — which is the classic
/// credential-confusion spelling an allowlist exists to refuse.
///
/// Indexing is by `char`, never by byte, so a multi-byte character in the
/// authority can never be split mid-code-point.
fn authority_host(url: &str) -> Option<String> {
    let chars: Vec<char> = url.chars().collect();
    let colon = chars.iter().position(|&c| c == ':')?;
    let mut i = colon + 1;
    let mut slashes = 0;
    while i < chars.len() && (chars[i] == '/' || chars[i] == '\\') {
        slashes += 1;
        i += 1;
    }
    if slashes < 2 {
        return None;
    }
    let start = i;
    let mut j = i;
    while j < chars.len() && !matches!(chars[j], '/' | '\\' | '?' | '#') {
        j += 1;
    }
    let authority: Vec<char> = chars[start..j].to_vec();
    let after_userinfo: &[char] = match authority.iter().rposition(|&c| c == '@') {
        Some(at) => &authority[at + 1..],
        None => &authority[..],
    };
    if after_userinfo.is_empty() {
        return None;
    }
    if after_userinfo[0] == '[' {
        let close = after_userinfo.iter().position(|&c| c == ']')?;
        return Some(
            after_userinfo[..=close]
                .iter()
                .collect::<String>()
                .to_lowercase(),
        );
    }
    let end = after_userinfo
        .iter()
        .position(|&c| c == ':')
        .unwrap_or(after_userinfo.len());
    let host: String = after_userinfo[..end].iter().collect();
    let n = normalize_host(&host);
    if n.is_empty() { None } else { Some(n) }
}

/// Resolve a URL to the destination a policy reasons about. Runs the scheme
/// floor FIRST — there is nothing to say about where an unsafe URL points.
#[must_use]
pub fn classify_destination(url: &str) -> Destination {
    let Some(safe) = sanitize_url(url) else {
        return Destination::Rejected;
    };
    if safe.is_empty() {
        return Destination::Local;
    }
    match extract_scheme(&safe) {
        // No scheme reaching here is same-origin: the floor has already refused
        // every protocol-relative spelling, which is the one schemeless shape
        // that leaves the origin.
        None => Destination::Local,
        Some(scheme) if NETWORK_SCHEMES.contains(&scheme.as_str()) => match authority_host(&safe) {
            Some(h) => Destination::Remote(h),
            None => Destination::Rejected,
        },
        Some(scheme) => Destination::NonNetwork(scheme),
    }
}

/// The whole check: scheme floor, then destination policy, for one class.
#[must_use]
pub fn check_destination(policy: &EgressPolicy, class: EgressClass, url: &str) -> EgressVerdict {
    let normalised = || {
        sanitize_url(url)
            .map(|c| c.into_owned())
            .unwrap_or_default()
    };
    match classify_destination(url) {
        Destination::Rejected => EgressVerdict::UnsafeUrl,
        Destination::Local => {
            if policy.allow_local {
                EgressVerdict::Allowed(normalised())
            } else {
                EgressVerdict::LocalDenied { class }
            }
        }
        Destination::NonNetwork(scheme) => {
            if policy.allow_non_network {
                EgressVerdict::Allowed(normalised())
            } else {
                EgressVerdict::NonNetworkDenied { scheme, class }
            }
        }
        Destination::Remote(host) => {
            if policy.is_declared_origin(class, &host) {
                EgressVerdict::Allowed(normalised())
            } else {
                EgressVerdict::UndeclaredOrigin { host, class }
            }
        }
    }
}

/// The `href` / `src` a REFUSED destination renders as.
///
/// Deliberately NOT the bare `about:blank` the scheme floor emits: a silent
/// neuter is indistinguishable from an authoring mistake, and "nothing
/// happened" and "this was refused" are different facts. The fragment is inert
/// in every browser and greppable in a rendered document.
pub const EGRESS_REFUSAL_URL: &str = "about:blank#fuaran-egress-refused";

/// The attribute name an emission site attaches beside a refused destination.
/// Passes [`super::sanitize::is_allowed_extra_attribute_key`] by construction.
pub const EGRESS_REFUSAL_ATTRIBUTE: &str = "data-fuaran-egress-refused";

/// The refusal marker for a verdict, or `None` when the destination was
/// allowed. The VALUE names the class and — where there is one — the host; it
/// **never** carries the URL, because the query string of a refused
/// exfiltration attempt is the payload itself, and a refusal record that quoted
/// it would become the disclosure it exists to prevent.
#[must_use]
pub fn egress_refusal_marker(verdict: &EgressVerdict) -> Option<(&'static str, String)> {
    let value = match verdict {
        EgressVerdict::Allowed(_) => return None,
        EgressVerdict::UnsafeUrl => "unsafe-url".to_string(),
        EgressVerdict::UndeclaredOrigin { host, class } => format!("{}:{host}", class.name()),
        EgressVerdict::LocalDenied { class } => format!("{}:local", class.name()),
        EgressVerdict::NonNetworkDenied { scheme, class } => format!("{}:{scheme}", class.name()),
    };
    Some((EGRESS_REFUSAL_ATTRIBUTE, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared_example() -> EgressPolicy {
        deny_non_local_egress()
            .allow_origin(
                EgressOrigin::ExactHost("cdn.example".to_string()),
                &[EgressClass::Media],
            )
            .allow_origin(
                EgressOrigin::HostSuffix("docs.example".to_string()),
                &[EgressClass::Hyperlink],
            )
    }

    #[test]
    fn suffix_matches_at_a_label_boundary_never_as_a_substring() {
        let p = declared_example();
        for host in ["docs.example", "eu.docs.example", "a.b.docs.example"] {
            assert!(
                p.is_declared_origin(EgressClass::Hyperlink, host),
                "{host} should match the suffix rule"
            );
        }
        for host in ["notdocs.example", "docs.example.evil", "xdocs.example"] {
            assert!(
                !p.is_declared_origin(EgressClass::Hyperlink, host),
                "{host} must NOT match the suffix rule"
            );
        }
    }

    #[test]
    fn exact_host_does_not_match_subdomains() {
        let p = declared_example();
        assert!(p.is_declared_origin(EgressClass::Media, "cdn.example"));
        assert!(!p.is_declared_origin(EgressClass::Media, "a.cdn.example"));
        assert!(!p.is_declared_origin(EgressClass::Media, "notcdn.example"));
    }

    #[test]
    fn rules_are_class_scoped() {
        let p = declared_example();
        // cdn.example is declared for media only; docs.example for hyperlink only.
        assert!(p.is_declared_origin(EgressClass::Media, "cdn.example"));
        assert!(!p.is_declared_origin(EgressClass::Hyperlink, "cdn.example"));
        assert!(p.is_declared_origin(EgressClass::Hyperlink, "docs.example"));
        assert!(!p.is_declared_origin(EgressClass::Media, "docs.example"));
        // Neither is declared for a class no rule names.
        assert!(!p.is_declared_origin(EgressClass::Route, "cdn.example"));
        assert!(!p.is_declared_origin(EgressClass::Download, "docs.example"));
    }

    #[test]
    fn an_empty_class_list_on_a_rule_permits_nothing() {
        let p = EgressPolicy {
            rules: vec![EgressRule {
                origin: EgressOrigin::ExactHost("cdn.example".to_string()),
                classes: Vec::new(),
            }],
            ..deny_non_local_egress()
        };
        for class in EgressClass::ALL {
            assert!(!p.is_declared_origin(class, "cdn.example"));
        }
        assert!(!p.has_non_local_egress());
        // …whereas an empty class list on the CONSTRUCTOR means every class.
        let q = deny_non_local_egress()
            .allow_origin(EgressOrigin::ExactHost("cdn.example".to_string()), &[]);
        for class in EgressClass::ALL {
            assert!(q.is_declared_origin(class, "cdn.example"));
        }
        assert!(q.has_non_local_egress());
    }

    #[test]
    fn userinfo_is_discarded_before_the_last_at_sign() {
        // The credential-confusion spelling: this is a request to evil.example.
        assert_eq!(
            classify_destination("https://good.example@evil.example/x"),
            Destination::Remote("evil.example".to_string())
        );
        assert_eq!(
            classify_destination("https://a@b@evil.example/x"),
            Destination::Remote("evil.example".to_string())
        );
        // …and a policy declaring the decoy host refuses it.
        let p = deny_non_local_egress()
            .allow_origin(EgressOrigin::ExactHost("good.example".to_string()), &[]);
        assert_eq!(
            check_destination(
                &p,
                EgressClass::Hyperlink,
                "https://good.example@evil.example/x"
            ),
            EgressVerdict::UndeclaredOrigin {
                host: "evil.example".to_string(),
                class: EgressClass::Hyperlink,
            }
        );
    }

    #[test]
    fn a_trailing_root_dot_is_the_same_host() {
        assert_eq!(
            classify_destination("https://cdn.example./p.png"),
            Destination::Remote("cdn.example".to_string())
        );
        let p = declared_example();
        assert!(p.is_declared_origin(EgressClass::Media, "cdn.example."));
        assert!(p.is_declared_origin(EgressClass::Hyperlink, "EU.Docs.Example."));
        // A rule spelled with the root dot normalises the same way.
        let q = deny_non_local_egress().allow_origin(
            EgressOrigin::ExactHost("cdn.example.".to_string()),
            &[EgressClass::Media],
        );
        assert!(q.is_declared_origin(EgressClass::Media, "cdn.example"));
    }

    #[test]
    fn ports_and_backslashes_and_ipv6_literals() {
        assert_eq!(
            classify_destination("https://cdn.example:8443/p.png"),
            Destination::Remote("cdn.example".to_string())
        );
        // WHATWG folds `\` into `/` when locating the authority.
        assert_eq!(
            classify_destination(r"https:\\cdn.example\p.png"),
            Destination::Remote("cdn.example".to_string())
        );
        assert_eq!(
            classify_destination("https://[2001:db8::1]:443/x"),
            Destination::Remote("[2001:db8::1]".to_string())
        );
    }

    #[test]
    fn the_scheme_floor_runs_first_and_hostless_schemes_are_non_network() {
        assert_eq!(
            classify_destination("javascript:alert(1)"),
            Destination::Rejected
        );
        assert_eq!(
            classify_destination("//evil.example/x"),
            Destination::Rejected
        );
        assert_eq!(classify_destination(""), Destination::Local);
        assert_eq!(classify_destination("/guide#top"), Destination::Local);
        assert_eq!(classify_destination("#frag"), Destination::Local);
        assert_eq!(
            classify_destination("mailto:a@b.example"),
            Destination::NonNetwork("mailto".to_string())
        );
        assert_eq!(
            classify_destination("tel:+441234"),
            Destination::NonNetwork("tel".to_string())
        );
        // A network scheme with no extractable host has nowhere a rule could
        // name, so it is refused rather than treated as same-origin.
        assert_eq!(classify_destination("https://"), Destination::Rejected);
        assert_eq!(classify_destination("https://@/x"), Destination::Rejected);
        assert_eq!(
            classify_destination("https://:8443/x"),
            Destination::Rejected
        );
        // …but a RUN of leading separators is consumed whole, exactly as the
        // reference host does: `https:///x` names the host `x`, it is not a
        // hostless URL. Pinned because the obvious reading of the third slash —
        // "an empty authority" — is the one that would silently turn an
        // off-origin request into a same-origin one.
        assert_eq!(
            classify_destination("https:///x"),
            Destination::Remote("x".to_string())
        );
    }

    #[test]
    fn refusal_markers_carry_the_class_and_the_host_never_the_query() {
        let p = deny_non_local_egress();
        let v = check_destination(
            &p,
            EgressClass::Media,
            "https://collector.example/p.png?who=me",
        );
        assert_eq!(
            egress_refusal_marker(&v),
            Some((EGRESS_REFUSAL_ATTRIBUTE, "media:collector.example".into()))
        );
        let v = check_destination(&p, EgressClass::Hyperlink, "mailto:a@collector.example");
        assert_eq!(
            egress_refusal_marker(&v),
            Some((EGRESS_REFUSAL_ATTRIBUTE, "hyperlink:mailto".into()))
        );
        let v = check_destination(&p, EgressClass::Hyperlink, "javascript:alert(1)");
        assert_eq!(
            egress_refusal_marker(&v),
            Some((EGRESS_REFUSAL_ATTRIBUTE, "unsafe-url".into()))
        );
        let denies_local = EgressPolicy {
            allow_local: false,
            ..deny_non_local_egress()
        };
        let v = check_destination(&denies_local, EgressClass::Hyperlink, "/guide#top");
        assert_eq!(
            egress_refusal_marker(&v),
            Some((EGRESS_REFUSAL_ATTRIBUTE, "hyperlink:local".into()))
        );
        // An allowed destination carries no marker at all.
        assert_eq!(
            egress_refusal_marker(&check_destination(
                &permissive_egress(),
                EgressClass::Hyperlink,
                "https://collector.example/x?s=secret"
            )),
            None
        );
    }

    #[test]
    fn class_wire_spellings_round_trip() {
        for class in EgressClass::ALL {
            assert_eq!(EgressClass::parse(class.name()), Some(class));
        }
        assert_eq!(
            EgressClass::parse(" FILEREAD "),
            Some(EgressClass::FileRead)
        );
        assert_eq!(EgressClass::parse("navigate"), None);
        assert_eq!(EgressClass::parse(""), None);
    }

    #[test]
    fn the_shipped_policies_say_what_they_are_named() {
        let deny = deny_non_local_egress();
        assert!(!deny.has_non_local_egress());
        assert!(matches!(
            check_destination(&deny, EgressClass::Hyperlink, "https://any.example/x"),
            EgressVerdict::UndeclaredOrigin { .. }
        ));
        assert!(matches!(
            check_destination(&deny, EgressClass::Hyperlink, "/guide"),
            EgressVerdict::Allowed(_)
        ));
        let permissive = permissive_egress();
        assert!(permissive.has_non_local_egress());
        for url in [
            "https://any.example/x",
            "mailto:a@b.example",
            "/guide",
            "#frag",
        ] {
            assert!(
                matches!(
                    check_destination(&permissive, EgressClass::Media, url),
                    EgressVerdict::Allowed(_)
                ),
                "permissive should allow {url}"
            );
        }
    }
}
