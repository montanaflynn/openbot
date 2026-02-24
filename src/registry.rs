//! Client for the skills.sh registry and GitHub raw content fetching.

use anyhow::{Context, Result};
use serde::Deserialize;

/// A skill returned by the skills.sh search API.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct RegistrySkill {
    /// Full identifier, e.g. "obra/superpowers/brainstorming".
    pub id: String,
    /// Short skill name, e.g. "brainstorming".
    #[serde(rename = "skillId")]
    pub skill_id: String,
    /// Display name.
    pub name: String,
    /// Number of installs.
    pub installs: u64,
    /// Source repository, e.g. "obra/superpowers".
    pub source: String,
}

/// Response from `GET https://skills.sh/api/search`.
#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    /// Skills returned in the current page of search results.
    pub skills: Vec<RegistrySkill>,
    /// Total number of matches reported by the API.
    pub count: u64,
}

/// Search the skills.sh registry.
pub async fn search(query: &str, limit: u32) -> Result<SearchResponse> {
    let url = format!(
        "https://skills.sh/api/search?q={}&limit={limit}",
        urlencoded(query),
    );

    let resp = reqwest::get(&url)
        .await
        .context("requesting skills.sh search API")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("skills.sh API returned {status}: {body}");
    }

    resp.json::<SearchResponse>()
        .await
        .context("parsing skills.sh search response")
}

/// Fetch a skill's SKILL.md content from GitHub.
///
/// Tries the multi-skill repo layout first:
///   `https://raw.githubusercontent.com/{source}/main/skills/{skill_id}/SKILL.md`
///
/// Falls back to single-skill repo root:
///   `https://raw.githubusercontent.com/{source}/main/SKILL.md`
pub async fn fetch_skill_md(source: &str, skill_id: &str) -> Result<String> {
    let client = reqwest::Client::new();

    // Try multi-skill layout first.
    let multi_url =
        format!("https://raw.githubusercontent.com/{source}/main/skills/{skill_id}/SKILL.md");

    let resp = client
        .get(&multi_url)
        .send()
        .await
        .context("fetching SKILL.md from GitHub")?;

    if resp.status().is_success() {
        return resp.text().await.context("reading SKILL.md body");
    }

    // Fallback: single-skill repo.
    let single_url = format!("https://raw.githubusercontent.com/{source}/main/SKILL.md");

    let resp = client
        .get(&single_url)
        .send()
        .await
        .context("fetching SKILL.md (fallback) from GitHub")?;

    if resp.status().is_success() {
        return resp.text().await.context("reading SKILL.md body");
    }

    anyhow::bail!(
        "could not find SKILL.md for {skill_id} in {source} (tried multi-skill and root layouts)"
    )
}

/// Encode a subset of reserved characters for a query parameter value.
fn urlencoded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            ' ' => out.push_str("%20"),
            '&' => out.push_str("%26"),
            '=' => out.push_str("%3D"),
            '+' => out.push_str("%2B"),
            '#' => out.push_str("%23"),
            '%' => out.push_str("%25"),
            _ => out.push(ch),
        }
    }
    out
}
