//! Picking the release to update to from the GitHub Releases API answer (ADR-0185).

use semver::Version;
use serde::Deserialize;

use super::Channel;

/// One release of the API's list; only what the updater reads.
#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// A downloadable file of a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetRef {
    pub name: String,
    pub url: String,
    /// The URL of `<name>.sig`.
    pub sig_url: String,
}

/// A release newer than the running version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub version: Version,
    pub tag: String,
    pub page_url: String,
    /// The file for this platform with its signature; `None` when the release has none.
    pub asset: Option<AssetRef>,
}

/// The name of the release file for this platform, `None` where there is no package.
pub fn asset_name(tag: &str) -> Option<String> {
    asset_name_for(tag, std::env::consts::OS, std::env::consts::ARCH)
}

/// [`asset_name`] for `os` and `arch` as `std::env::consts` spells them.
pub fn asset_name_for(tag: &str, os: &str, arch: &str) -> Option<String> {
    match (os, arch) {
        ("linux", "x86_64") => Some(format!("SLAM-{tag}-x86_64.AppImage")),
        ("windows", "x86_64") => Some(format!("SLAM-{tag}-windows-x64.zip")),
        _ => None,
    }
}

/// The version a tag names: `v0.1.5-alpha` is 0.1.5-alpha.
pub fn parse_tag(tag: &str) -> Option<Version> {
    Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}

/// The API's answer could not be read.
#[derive(Debug)]
pub struct BadAnswer(pub String);

/// The newest release of `json` (the API's list) on `channel` that is newer than `current`.
/// Drafts and tags that are not versions are skipped; the stable channel also skips
/// pre-releases. `asset` is the file `asset_name` names, if the release has it and its
/// signature.
pub fn select(
    json: &[u8],
    channel: Channel,
    current: &Version,
    asset_name: impl Fn(&str) -> Option<String>,
) -> Result<Option<Found>, BadAnswer> {
    let releases: Vec<Release> =
        serde_json::from_slice(json).map_err(|e| BadAnswer(e.to_string()))?;
    let newest = releases
        .into_iter()
        .filter(|r| !r.draft)
        .filter(|r| channel == Channel::Prerelease || !r.prerelease)
        .filter_map(|r| parse_tag(&r.tag_name).map(|v| (v, r)))
        .filter(|(v, _)| channel == Channel::Prerelease || v.pre.is_empty())
        .max_by(|a, b| a.0.cmp(&b.0));
    let Some((version, release)) = newest.filter(|(v, _)| v > current) else {
        return Ok(None);
    };
    let wanted = asset_name(&release.tag_name);
    let find = |name: &str| release.assets.iter().find(|a| a.name == name);
    let asset = wanted.and_then(|name| {
        let file = find(&name)?;
        let sig = find(&format!("{name}.sig"))?;
        Some(AssetRef {
            name,
            url: file.browser_download_url.clone(),
            sig_url: sig.browser_download_url.clone(),
        })
    });
    Ok(Some(Found {
        version,
        page_url: release.html_url,
        tag: release.tag_name,
        asset,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Vec<u8> {
        let asset = |tag: &str, ext: &str| {
            format!(
                r#"{{"name":"SLAM-{tag}-x86_64.AppImage{ext}","browser_download_url":"https://x/{tag}{ext}"}}"#
            )
        };
        format!(
            r#"[
            {{"tag_name":"v0.1.6-alpha","draft":true,"prerelease":true,"html_url":"d","assets":[]}},
            {{"tag_name":"v0.1.5-alpha","draft":false,"prerelease":true,"html_url":"p5","assets":[{},{}]}},
            {{"tag_name":"v0.1.4-alpha","draft":false,"prerelease":true,"html_url":"p4","assets":[]}},
            {{"tag_name":"v0.1.0","draft":false,"prerelease":false,"html_url":"s1","assets":[]}},
            {{"tag_name":"nightly","draft":false,"prerelease":false,"html_url":"n","assets":[]}}
            ]"#,
            asset("v0.1.5-alpha", ""),
            asset("v0.1.5-alpha", ".sig"),
        )
        .into_bytes()
    }

    fn name(tag: &str) -> Option<String> {
        asset_name_for(tag, "linux", "x86_64")
    }

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn the_newest_non_draft_prerelease_wins_on_the_prerelease_channel() {
        let found = select(&list(), Channel::Prerelease, &v("0.1.4-alpha"), name)
            .unwrap()
            .unwrap();
        assert_eq!(found.tag, "v0.1.5-alpha");
        assert_eq!(found.page_url, "p5");
        let asset = found.asset.unwrap();
        assert_eq!(asset.name, "SLAM-v0.1.5-alpha-x86_64.AppImage");
        assert_eq!(asset.sig_url, "https://x/v0.1.5-alpha.sig");
    }

    #[test]
    fn the_stable_channel_skips_prereleases() {
        // 0.1.0 is older than the running 0.1.4-alpha.
        assert_eq!(
            select(&list(), Channel::Stable, &v("0.1.4-alpha"), name).unwrap(),
            None
        );
        let found = select(&list(), Channel::Stable, &v("0.0.9"), name)
            .unwrap()
            .unwrap();
        assert_eq!(found.tag, "v0.1.0");
        assert_eq!(found.asset, None);
    }

    #[test]
    fn nothing_is_offered_when_up_to_date_or_ahead() {
        for current in ["0.1.5-alpha", "0.2.0"] {
            assert_eq!(
                select(&list(), Channel::Prerelease, &v(current), name).unwrap(),
                None
            );
        }
    }

    #[test]
    fn a_release_beats_its_own_alpha() {
        let json = br#"[{"tag_name":"v0.2.0","prerelease":false},{"tag_name":"v0.2.0-alpha","prerelease":true}]"#;
        let found = select(json, Channel::Prerelease, &v("0.2.0-alpha"), name)
            .unwrap()
            .unwrap();
        assert_eq!(found.tag, "v0.2.0");
    }

    #[test]
    fn a_missing_signature_means_no_installable_asset() {
        let json = br#"[{"tag_name":"v0.3.0-alpha","prerelease":true,"assets":[
            {"name":"SLAM-v0.3.0-alpha-x86_64.AppImage","browser_download_url":"u"}]}]"#;
        let found = select(json, Channel::Prerelease, &v("0.1.0"), name)
            .unwrap()
            .unwrap();
        assert_eq!(found.asset, None);
    }

    #[test]
    fn bad_json_is_an_error() {
        assert!(select(b"{", Channel::Prerelease, &v("0.1.0"), name).is_err());
    }

    #[test]
    fn asset_names_follow_the_platform() {
        assert_eq!(
            asset_name_for("v1.0.0", "windows", "x86_64").as_deref(),
            Some("SLAM-v1.0.0-windows-x64.zip")
        );
        assert_eq!(
            asset_name_for("v1.0.0", "linux", "x86_64").as_deref(),
            Some("SLAM-v1.0.0-x86_64.AppImage")
        );
        assert_eq!(asset_name_for("v1.0.0", "macos", "aarch64"), None);
    }
}
