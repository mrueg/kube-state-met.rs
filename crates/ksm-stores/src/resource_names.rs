//! Resource-name helpers of `internal/store/utils.go` (`isHugePageResourceName`,
//! `isAttachableVolumeResourceName`, `isExtendedResourceName`) with the parts
//! of `k8s.io/apimachinery` validation they rely on.

/// `v1.ResourceHugePagesPrefix`.
const RESOURCE_HUGE_PAGES_PREFIX: &str = "hugepages-";
/// `v1.ResourceAttachableVolumesPrefix`.
const RESOURCE_ATTACHABLE_VOLUMES_PREFIX: &str = "attachable-volumes-";
/// `v1.DefaultResourceRequestsPrefix`.
const DEFAULT_RESOURCE_REQUESTS_PREFIX: &str = "requests.";
/// `v1.ResourceDefaultNamespacePrefix`.
const RESOURCE_DEFAULT_NAMESPACE_PREFIX: &str = "kubernetes.io/";

/// `isHugePageResourceName`.
pub fn is_huge_page_resource_name(name: &str) -> bool {
    name.starts_with(RESOURCE_HUGE_PAGES_PREFIX)
}

/// `isAttachableVolumeResourceName`.
pub fn is_attachable_volume_resource_name(name: &str) -> bool {
    name.starts_with(RESOURCE_ATTACHABLE_VOLUMES_PREFIX)
}

/// `isExtendedResourceName`.
pub fn is_extended_resource_name(name: &str) -> bool {
    if is_native_resource(name) || name.starts_with(DEFAULT_RESOURCE_REQUESTS_PREFIX) {
        return false;
    }
    // Ensure it satisfies the rules in IsQualifiedName() after converted into
    // quota resource name.
    let name_for_quota = format!("{DEFAULT_RESOURCE_REQUESTS_PREFIX}{name}");
    is_qualified_name(&name_for_quota)
}

/// `isNativeResource`.
fn is_native_resource(name: &str) -> bool {
    !name.contains('/') || is_prefixed_native_resource(name)
}

/// `isPrefixedNativeResource`.
fn is_prefixed_native_resource(name: &str) -> bool {
    name.contains(RESOURCE_DEFAULT_NAMESPACE_PREFIX)
}

/// `labelKeyMaxLength`.
const LABEL_KEY_MAX_LENGTH: usize = 63;
/// `DNS1123SubdomainMaxLength`.
const DNS1123_SUBDOMAIN_MAX_LENGTH: usize = 253;

/// Whether `validation.IsQualifiedName` (`content.IsLabelKey`) returns no
/// errors.
fn is_qualified_name(value: &str) -> bool {
    let parts: Vec<&str> = value.split('/').collect();
    let name = match parts.as_slice() {
        [name] => *name,
        [prefix, name] => {
            if prefix.is_empty() || !is_dns1123_subdomain(prefix) {
                return false;
            }
            *name
        }
        _ => return false,
    };
    !name.is_empty() && name.len() <= LABEL_KEY_MAX_LENGTH && matches_label_key(name)
}

/// `^([A-Za-z0-9][-A-Za-z0-9_.]*)?[A-Za-z0-9]$`.
fn matches_label_key(name: &str) -> bool {
    let b = name.as_bytes();
    match (b.first(), b.last()) {
        (Some(f), Some(l)) => {
            f.is_ascii_alphanumeric()
                && l.is_ascii_alphanumeric()
                && b.iter()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        }
        _ => false,
    }
}

/// Whether `content.IsDNS1123Subdomain` returns no errors: at most 253 bytes
/// matching `^[a-z0-9]([-a-z0-9]*[a-z0-9])?(\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*$`.
fn is_dns1123_subdomain(value: &str) -> bool {
    value.len() <= DNS1123_SUBDOMAIN_MAX_LENGTH && value.split('.').all(matches_dns1123_label)
}

/// `^[a-z0-9]([-a-z0-9]*[a-z0-9])?$`.
fn matches_dns1123_label(label: &str) -> bool {
    let lower_alnum = |c: &u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    let b = label.as_bytes();
    match (b.first(), b.last()) {
        (Some(f), Some(l)) => {
            lower_alnum(f) && lower_alnum(l) && b.iter().all(|c| lower_alnum(c) || *c == b'-')
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended() {
        assert!(is_extended_resource_name("nvidia.com/gpu"));
        assert!(is_extended_resource_name("example.com/a_b.c"));
        assert!(!is_extended_resource_name("cpu"));
        assert!(!is_extended_resource_name("kubernetes.io/foo"));
        assert!(!is_extended_resource_name("requests.foo/bar"));
        assert!(!is_extended_resource_name("Example.com/gpu"));
        assert!(!is_extended_resource_name("a/b/c"));
        assert!(!is_extended_resource_name("example.com/-gpu"));
        assert!(!is_extended_resource_name("example..com/gpu"));
        assert!(!is_extended_resource_name("example.com/"));
    }

    #[test]
    fn prefixes() {
        assert!(is_huge_page_resource_name("hugepages-2Mi"));
        assert!(is_attachable_volume_resource_name(
            "attachable-volumes-aws-ebs"
        ));
        assert!(!is_huge_page_resource_name("memory"));
    }
}
