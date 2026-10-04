//! Which network endpoints this product refuses to reach.
//!
//! Two layers have to answer the same question and neither can borrow code
//! from the other: `OhosPlatform::open_url` hands a URL to the system
//! browser, which resolves it outside this process, and the `getaddrinfo`
//! wrapper in this crate intercepts name resolution for every socket in the
//! final cdylib. Both consult the predicates below so the blocked set has a
//! single definition.
//!
//! A host is blocked when one of its dot-separated labels is `zed` or starts
//! with `zed-`. That covers `zed.dev` and every subdomain of it as well as
//! `zed-industries.com`, while leaving hosts that merely happen to contain
//! the same letters alone (`maxtrauthzed.com`, `zedsite.com`).

/// Separator every hierarchical URL scheme ends with, as in `https://`.
/// A string without it is not a hierarchical URL and never reaches the
/// resolver, so `mailto:` and application-private schemes such as `zed://`
/// are left alone.
const SCHEME_SEPARATOR: &str = "://";

/// The characters that end the authority component of a URL.
const AUTHORITY_TERMINATORS: [char; 3] = ['/', '?', '#'];

/// Host label that marks an endpoint of the upstream vendor.
const BLOCKED_LABEL: &str = "zed";

/// What else makes a host label count as one of the vendor's: it is what
/// turns `zed-industries` into a vendor label as well.
const BLOCKED_LABEL_PREFIX: &str = "zed-";

/// Whether a hostname belongs to the upstream vendor. The port suffix is not
/// part of the name, and IPv6 literals contain colons too - truncating those
/// can only yield a fragment that fails the label test below.
pub fn host_is_blocked(host: &str) -> bool {
    let name = host.split(':').next().unwrap_or(host);
    name.split('.').any(is_blocked_label)
}

/// Whether the host a URL points at belongs to the upstream vendor. Paths and
/// query strings are not inspected: `github.com/zed-industries/zed` names
/// GitHub, not the vendor.
pub fn url_host_is_blocked(url: &str) -> bool {
    let Some((_, authority)) = url.split_once(SCHEME_SEPARATOR) else {
        return false;
    };
    let authority = authority
        .split(AUTHORITY_TERMINATORS)
        .next()
        .unwrap_or(authority);
    let host = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    host_is_blocked(host)
}

/// Host names are case-insensitive, so the labels are compared folded.
fn is_blocked_label(label: &str) -> bool {
    let label = label.to_ascii_lowercase();
    label == BLOCKED_LABEL || label.starts_with(BLOCKED_LABEL_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::{host_is_blocked, url_host_is_blocked};

    #[test]
    fn blocks_the_vendor_domain_and_its_subdomains() {
        assert!(host_is_blocked("zed.dev"));
        assert!(host_is_blocked("api.zed.dev"));
        assert!(host_is_blocked("ZED.DEV"));
        assert!(host_is_blocked("zed.dev:443"));
    }

    #[test]
    fn blocks_the_vendor_organisation_domain() {
        assert!(host_is_blocked("zed-industries.com"));
        assert!(host_is_blocked("api.zed-industries.com"));
    }

    #[test]
    fn keeps_hosts_that_only_contain_the_letters() {
        assert!(!host_is_blocked("maxtrauthzed.com"));
        assert!(!host_is_blocked("zedsite.com"));
        assert!(!host_is_blocked("github.com"));
        assert!(!host_is_blocked("localhost"));
        assert!(!host_is_blocked(""));
    }

    #[test]
    fn reads_the_host_out_of_a_url() {
        assert!(url_host_is_blocked(
            "https://zed.dev/docs/ai/privacy-and-security"
        ));
        assert!(url_host_is_blocked("https://api.zed.dev/"));
        assert!(url_host_is_blocked("https://user@zed.dev/"));
        assert!(url_host_is_blocked("http://ZED.DEV?q=1"));
    }

    #[test]
    fn keeps_urls_that_do_not_name_a_vendor_host() {
        assert!(!url_host_is_blocked(
            "https://github.com/zed-industries/zed"
        ));
        assert!(!url_host_is_blocked("mailto:hi@zed.dev"));
        assert!(!url_host_is_blocked(
            "zed://git/clone?repo=https://github.com/zed-industries/zed"
        ));
        assert!(!url_host_is_blocked("https://twitter.com/zeddotdev"));
        assert!(!url_host_is_blocked("file:///storage/Users/currentUser/a.txt"));
        assert!(!url_host_is_blocked("relative/path"));
    }
}