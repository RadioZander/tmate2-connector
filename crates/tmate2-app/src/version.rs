//! Which build this is, for the About tab, `--version` and the log.
//!
//! A release shows its version (`0.3.0`). A build from later commits shows
//! how many commits it is past that release and the commit itself
//! (`0.3.0+4 (08dc2d5)`), and a build with uncommitted changes adds
//! `modified`. `build.rs` records `git describe` at build time; without git
//! (a source copy with no history) only the package version is shown,
//! unless `TMATE2_BUILD` was set to a `git describe` line for the build.

use std::sync::LazyLock;

/// This build's label.
pub static BUILD: LazyLock<String> =
    LazyLock::new(|| label(env!("CARGO_PKG_VERSION"), env!("TMATE2_DESCRIBE")));

/// The label for a `git describe --tags --long --dirty` line
/// (`v0.3.0-4-g08dc2d5[-dirty]`), or `version` if there isn't one.
fn label(version: &str, describe: &str) -> String {
    let (describe, modified) = match describe.strip_suffix("-dirty") {
        Some(d) => (d, true),
        None => (describe, false),
    };
    let mut parts = describe.rsplitn(3, '-');
    let (Some(hash), Some(ahead), Some(tag)) = (parts.next(), parts.next(), parts.next()) else {
        return version.to_string();
    };
    let (Some(hash), Ok(ahead)) = (hash.strip_prefix('g'), ahead.parse::<u32>()) else {
        return version.to_string();
    };
    // The tag, not the package version: between a version bump and its tag
    // the count is still from the last release.
    let mut s = tag.strip_prefix('v').unwrap_or(tag).to_string();
    if ahead > 0 {
        s.push_str(&format!("+{ahead} ({hash})"));
    } else if modified {
        s.push_str(&format!(" ({hash})"));
    }
    if modified {
        s.push_str(" modified");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::label;

    #[test]
    fn a_release_shows_its_version() {
        assert_eq!(label("0.3.0", "v0.3.0-0-g4151dd9"), "0.3.0");
    }

    #[test]
    fn later_commits_show_the_count_and_the_commit() {
        assert_eq!(label("0.3.0", "v0.3.0-4-g08dc2d5"), "0.3.0+4 (08dc2d5)");
        // A version bumped but not yet tagged still counts from the release.
        assert_eq!(label("0.4.0", "v0.3.0-5-gabc1234"), "0.3.0+5 (abc1234)");
    }

    #[test]
    fn uncommitted_changes_say_so() {
        assert_eq!(
            label("0.3.0", "v0.3.0-4-g08dc2d5-dirty"),
            "0.3.0+4 (08dc2d5) modified"
        );
        assert_eq!(
            label("0.3.0", "v0.3.0-0-g4151dd9-dirty"),
            "0.3.0 (4151dd9) modified"
        );
    }

    #[test]
    fn without_git_the_package_version() {
        assert_eq!(label("0.3.0", ""), "0.3.0");
        assert_eq!(label("0.3.0", "08dc2d5"), "0.3.0");
        assert_eq!(label("0.3.0", "v0.3.0-x-g08dc2d5"), "0.3.0");
    }

    #[test]
    fn a_tag_with_dashes_is_kept_whole() {
        assert_eq!(label("0.4.0", "v0.4.0-rc1-2-g1234567"), "0.4.0-rc1+2 (1234567)");
    }
}
