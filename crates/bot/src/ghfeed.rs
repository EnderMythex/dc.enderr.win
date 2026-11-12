//! GitHub in Discord (ported from the Misty bot): new releases announced
//! with their notes and download buttons, and the activity of repositories
//! (commits, pull requests, issues, stars, forks, tags and branches).
//!
//! GitHub's "events" API can be hours late, so the repositories are read
//! directly and compared with the previous check. The first check of a
//! repository only records where it stands: nothing old is announced.

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use dc_core::guild::Releases;

use crate::{
    app::App,
    github::{self, Actor, Release, Ticket},
    ui::{self, Msg},
};

const RELEASE_EVERY: Duration = Duration::from_secs(120);
const ACTIVITY_EVERY: Duration = Duration::from_secs(60);

/// Without a token (60 requests per hour for the whole bot), check less often.
fn pace(app: &App, normal: Duration, without_token: u64) -> Duration {
    if app.github.authenticated() {
        normal
    } else {
        normal.max(Duration::from_secs(without_token))
    }
}

fn log_error(what: &str, error: &anyhow::Error) {
    if !github::is_rate_limited(error) {
        tracing::warn!("{what}: {error:#}");
    }
}

/// Servers the bot is in, with their configuration.
fn servers(app: &App) -> Vec<(String, Arc<dc_core::guild::GuildConfig>)> {
    let ids: Vec<String> = app.guilds.lock().unwrap().keys().cloned().collect();
    ids.into_iter()
        .map(|id| {
            let config = app.config(&id);
            (id, config)
        })
        .collect()
}

fn repo_name(repo: &str) -> &str {
    repo.rsplit('/').next().unwrap_or(repo)
}

// ---------------------------------------------------------------- releases

/// Release notes for Discord: headings one level smaller, bounded length.
fn notes(body: &str, max: usize) -> String {
    let text: String = body
        .replace("\r\n", "\n")
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if let Some(h) = trimmed
                .strip_prefix("# ")
                .or_else(|| trimmed.strip_prefix("## "))
            {
                format!("### {h}")
            } else if let Some(h) = trimmed.strip_prefix("#### ") {
                format!("**{h}**")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    ui::truncate(text.trim(), max)
}

fn size(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

/// `-# Setup.exe 9.0 MB · Portable.zip 10.0 MB` (three files at most).
fn files_line(release: &Release) -> Option<String> {
    let parts: Vec<String> = release
        .assets
        .iter()
        .take(3)
        .map(|asset| format!("{} {}", ui::escape(&asset.name), size(asset.size)))
        .collect();
    (!parts.is_empty()).then(|| format!("-# {}", parts.join("  ·  ")))
}

fn download_link(settings: &Releases, release: &Release) -> String {
    if !settings.download_url.is_empty() {
        settings.download_url.clone()
    } else {
        release.html_url.clone()
    }
}

/// `2026-11-11T02:52:10Z` -> `Nov 11, 2026, 02:52 UTC`.
fn release_date(iso: Option<&str>) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let Some(iso) = iso.filter(|iso| github::unix(Some(iso)).is_some()) else {
        return String::new();
    };
    let month = iso
        .get(5..7)
        .and_then(|m| m.parse::<usize>().ok())
        .and_then(|m| MONTHS.get(m.wrapping_sub(1)))
        .copied()
        .unwrap_or("");
    let day = iso.get(8..10).unwrap_or("").trim_start_matches('0');
    format!(
        "{month} {day}, {}, {} UTC",
        iso.get(..4).unwrap_or(""),
        iso.get(11..16).unwrap_or("")
    )
}

/// Announcement of a version, like the Misty bot: banner (version and
/// date), notes, files, buttons; the role is mentioned above the card.
/// `mention`: announcements, not `/release`.
pub fn release_card(settings: &Releases, repo: &str, release: &Release, mention: bool) -> Msg {
    let title = format!("{} {}", repo_name(repo), release.tag_name);
    let label = if release.prerelease {
        "Pre-release"
    } else {
        "New version"
    };
    let mut extra = Vec::new();
    let role = settings.role.as_deref().filter(|_| mention);
    let body = release
        .body
        .as_deref()
        .map(|body| notes(body, 1800))
        .filter(|body| !body.is_empty())
        .unwrap_or_else(|| "The changes of this version are in its release notes.".into());
    extra.push(ui::text(body));
    extra.push(ui::separator());
    if let Some(files) = files_line(release) {
        extra.push(ui::text(files));
    }
    extra.push(ui::row(vec![
        ui::link_button("Download", &download_link(settings, release)),
        ui::link_button("Release notes", &release.html_url),
    ]));
    let default = if release.prerelease {
        ui::AMBER
    } else {
        0x6F9CFF
    };
    let color = settings.look.color_or(default);
    let svg = dc_core::cards::hero(
        label,
        &title,
        &release_date(release.published_at.as_deref()),
        color,
    );
    let fallback = vec![ui::text(format!("# {}", ui::escape(&title)))];
    let mut msg = Msg::with_image(color, "release.png", svg, extra, fallback);
    if let Some(role) = role {
        msg = msg.prepend(ui::text(format!("<@&{role}>")));
        msg.ping_roles = vec![role.to_owned()];
    }
    msg
}

/// `/download`: the latest version and its links.
pub fn download_card(settings: &Releases, repo: &str, latest: Option<&Release>) -> Msg {
    let mut children = vec![ui::text(format!("## Download {}", repo_name(repo)))];
    match latest {
        Some(release) => {
            let when = github::unix(release.published_at.as_deref())
                .map(|t| format!(" · <t:{t}:R>"))
                .unwrap_or_default();
            children.push(ui::text(format!(
                "**Latest version** `{}`{when}\n**Downloads** {}",
                release.tag_name,
                ui::number(release.downloads())
            )));
            if let Some(files) = files_line(release) {
                children.push(ui::text(files));
            }
            let mut buttons = vec![ui::link_button(
                "Download",
                &download_link(settings, release),
            )];
            if !settings.website.is_empty() {
                buttons.push(ui::link_button("Website", &settings.website));
            }
            buttons.push(ui::link_button("GitHub", &release.html_url));
            children.push(ui::row(buttons));
        }
        None => children.push(ui::text(
            "GitHub cannot be reached right now, or there is no release yet.",
        )),
    }
    Msg::card(ui::BLUE, children)
}

/// The repository announced on this server, if set up.
pub fn release_repo(settings: &Releases) -> Option<&str> {
    Some(settings.repo.as_str()).filter(|repo| !repo.is_empty())
}

async fn check_releases(app: &App) {
    let servers = servers(app);
    let repos: BTreeSet<String> = servers
        .iter()
        .filter(|(_, c)| c.releases.enabled && c.releases.channel.is_some())
        .filter_map(|(_, c)| release_repo(&c.releases).map(str::to_owned))
        .collect();
    for repo in repos {
        let list = match app.github.releases(&repo).await {
            Ok(list) => list,
            Err(error) => {
                log_error(&format!("releases of {repo}"), &error);
                continue;
            }
        };
        let Some(max) = list.iter().map(|r| r.id).max() else {
            continue;
        };
        let seen = app.watch.update(|s| {
            s.releases
                .insert(repo.clone(), max.max(*s.releases.get(&repo).unwrap_or(&0)))
        });
        let Some(seen) = seen else {
            tracing::info!(repo, "releases: starting point recorded");
            continue;
        };
        let mut fresh: Vec<&Release> = list.iter().filter(|r| r.id > seen).collect();
        fresh.sort_by_key(|r| r.id);
        for release in fresh {
            tracing::info!(repo, tag = release.tag_name, "new release");
            for (guild, config) in &servers {
                let settings = &config.releases;
                let Some(channel) = settings.channel.as_deref() else {
                    continue;
                };
                if !settings.enabled
                    || settings.repo != repo
                    || (release.prerelease && !settings.prereleases)
                {
                    continue;
                }
                let msg = release_card(settings, &repo, release, true);
                if let Err(error) = app.api.send_message(channel, &msg).await {
                    tracing::warn!(guild, %error, "announcing a release failed");
                }
            }
        }
    }
}

pub async fn watch_releases(app: Arc<App>) {
    loop {
        tokio::time::sleep(pace(&app, RELEASE_EVERY, 600)).await;
        check_releases(&app).await;
    }
}

// ---------------------------------------------------------------- activity

/// Something new in a repository, ready to be shown.
#[derive(Debug, Clone)]
pub struct Activity {
    /// One of `dc_core::guild::GITHUB_EVENTS`.
    pub kind: &'static str,
    pub repo: String,
    pub actor: Actor,
    pub title: String,
    pub details: Option<String>,
    pub url: String,
    pub color: u32,
}

const MERGED: u32 = 0xA371F7;
const STAR: u32 = 0xE3B341;
const GIT: u32 = 0x8B949E;

/// Default author: the owner of the repository.
fn owner(repo: &str) -> Actor {
    let login = repo.split('/').next().unwrap_or(repo).to_owned();
    Actor {
        avatar_url: format!("https://github.com/{login}.png"),
        login,
    }
}

fn actor_link(actor: &Actor) -> String {
    format!(
        "[**{0}**](https://github.com/{0})",
        actor.login.replace(['[', ']', '(', ')'], "")
    )
}

/// First line of a commit message, cleaned for Discord.
fn first_line(message: &str) -> String {
    let line = message
        .lines()
        .next()
        .unwrap_or("")
        .replace(['`', '[', ']'], "");
    ui::truncate(&line, 72)
}

pub fn card(activity: &Activity) -> Msg {
    let repo = &activity.repo;
    let mut text = format!(
        "-# [{repo}](https://github.com/{repo}) · <t:{}:R>\n{}",
        dc_core::now(),
        activity.title
    );
    if let Some(details) = &activity.details {
        text.push('\n');
        text.push_str(details);
    }
    Msg::card(
        activity.color,
        vec![
            ui::section_thumb(text, &activity.actor.avatar_url),
            ui::row(vec![ui::link_button("View on GitHub", &activity.url)]),
        ],
    )
}

fn push_activity(repo: &str, branch: &str, before: &str, fresh: &[github::Commit]) -> Activity {
    let head = &fresh[0];
    let actor = head.author.clone().unwrap_or_else(|| {
        let mut actor = owner(repo);
        if let Some(name) = head.commit.author.as_ref().and_then(|s| s.name.clone()) {
            actor.login = name;
        }
        actor
    });
    let n = fresh.len();
    let repo_url = format!("https://github.com/{repo}");
    let lines: Vec<String> = fresh
        .iter()
        .take(5)
        .map(|commit| {
            format!(
                "[`{}`]({repo_url}/commit/{}) {}",
                &commit.sha[..7.min(commit.sha.len())],
                commit.sha,
                first_line(&commit.commit.message)
            )
        })
        .collect();
    let mut details = lines.join("\n");
    if n > lines.len() {
        details.push_str(&format!("\n-# + {} more", n - lines.len()));
    }
    Activity {
        kind: "push",
        repo: repo.to_owned(),
        title: format!(
            "{} pushed **{n}** commit{} to `{branch}`",
            actor_link(&actor),
            if n > 1 { "s" } else { "" }
        ),
        actor,
        details: Some(details),
        url: format!(
            "{repo_url}/compare/{}...{}",
            &before[..before.len().min(12)],
            &head.sha[..head.sha.len().min(12)]
        ),
        color: 0x6F9CFF,
    }
}

/// New activity of a repository since the previous check, older first.
pub async fn poll(app: &App, repo: &str) -> anyhow::Result<Vec<Activity>> {
    let gh = &app.github;
    let info = gh.repo(repo).await?;
    let mut out = Vec::new();

    // Commits on the default branch.
    let branch = if info.default_branch.is_empty() {
        "main".to_owned()
    } else {
        info.default_branch.clone()
    };
    if let Ok(commits) = gh.commits(repo, &branch).await
        && let Some(head) = commits.first()
    {
        let seen = app.watch.read(|s| s.commits.get(repo).cloned());
        if let Some(seen) = seen
            && seen != head.sha
        {
            // Rewritten history: only the last commit, never an avalanche.
            let fresh: Vec<github::Commit> = match commits.iter().position(|c| c.sha == seen) {
                Some(index) => commits[..index].to_vec(),
                None => vec![head.clone()],
            };
            out.push(push_activity(repo, &branch, &seen, &fresh));
        }
        let sha = head.sha.clone();
        app.watch.update(|s| s.commits.insert(repo.to_owned(), sha));
    }

    // Pull requests and issues.
    let pulls = gh.pulls(repo).await.unwrap_or_default();
    let issues = gh.issues(repo).await.unwrap_or_default();
    let known = app.watch.read(|s| s.tickets.get(repo).cloned());
    let first = known.is_none();
    let mut known = known.unwrap_or_default();
    for (prefix, list) in [("pr", &pulls), ("issue", &issues)] {
        for ticket in list.iter().rev() {
            let key = format!("{prefix}-{}", ticket.number);
            let now = ticket.status().to_owned();
            let before = known.insert(key, now.clone());
            if first {
                continue;
            }
            let action = match (before.as_deref(), now.as_str()) {
                (None, "open") => "opened",
                (Some("open") | None, "merged") => "merged",
                (Some("open"), "closed") => "closed",
                (Some("closed" | "merged"), "open") => "reopened",
                _ => continue,
            };
            out.push(ticket_activity(app, repo, prefix, ticket, action).await);
        }
    }
    if known.len() > 300 {
        let mut keys: Vec<(u64, String)> = known
            .keys()
            .map(|k| {
                let n = k
                    .rsplit('-')
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                (n, k.clone())
            })
            .collect();
        keys.sort();
        let drop = known.len() - 300;
        for (_, key) in keys.iter().take(drop) {
            known.remove(key);
        }
    }
    app.watch
        .update(|s| s.tickets.insert(repo.to_owned(), known));

    // Forks.
    if let Ok(forks) = gh.forks(repo).await
        && let Some(max) = forks.iter().map(|f| f.id).max()
    {
        let seen = app.watch.read(|s| s.forks.get(repo).copied());
        if let Some(seen) = seen {
            let mut fresh: Vec<_> = forks.iter().filter(|f| f.id > seen).collect();
            fresh.sort_by_key(|f| f.id);
            for fork in fresh {
                out.push(Activity {
                    kind: "fork",
                    repo: repo.to_owned(),
                    title: format!(
                        "{} forked it to [`{1}`](https://github.com/{1})",
                        actor_link(&fork.owner),
                        fork.full_name
                    ),
                    actor: fork.owner.clone(),
                    details: None,
                    url: format!("https://github.com/{}", fork.full_name),
                    color: GIT,
                });
            }
        }
        app.watch
            .update(|s| s.forks.insert(repo.to_owned(), max.max(seen.unwrap_or(0))));
    }

    // Tags and branches.
    if let (Ok(tags), Ok(branches)) = (gh.tags(repo).await, gh.branches(repo).await) {
        let current: Vec<String> = tags
            .iter()
            .map(|t| format!("tag:{}", t.name))
            .chain(branches.iter().map(|b| format!("branch:{}", b.name)))
            .collect();
        let seen = app.watch.read(|s| s.refs.get(repo).cloned());
        if let Some(seen) = seen {
            for created in current.iter().filter(|r| !seen.contains(r)) {
                let (kind, name) = created.split_once(':').unwrap_or(("tag", created));
                let actor = owner(repo);
                out.push(Activity {
                    kind: "tag",
                    repo: repo.to_owned(),
                    title: format!(
                        "{} created {} `{}`",
                        actor_link(&actor),
                        if kind == "branch" { "branch" } else { "tag" },
                        name.replace('`', "")
                    ),
                    actor,
                    details: None,
                    url: format!("https://github.com/{repo}/tree/{name}"),
                    color: GIT,
                });
            }
        }
        app.watch
            .update(|s| s.refs.insert(repo.to_owned(), current));
    }

    // Stars: five cards at most at once.
    let count = info.stargazers_count;
    let seen = app.watch.read(|s| s.stars.get(repo).copied());
    if let Some(seen) = seen
        && count > seen
    {
        for n in (count - (count - seen).min(5) + 1)..=count {
            if let Ok(Some(star)) = gh.stargazer(repo, n).await {
                out.push(Activity {
                    kind: "star",
                    repo: repo.to_owned(),
                    title: format!("{} starred the repository", actor_link(&star.user)),
                    actor: star.user,
                    details: None,
                    url: format!("https://github.com/{repo}"),
                    color: STAR,
                });
            }
        }
    }
    app.watch.update(|s| s.stars.insert(repo.to_owned(), count));
    Ok(out)
}

/// A pull request or an issue; for a merge or a close, who did it.
async fn ticket_activity(
    app: &App,
    repo: &str,
    prefix: &str,
    ticket: &Ticket,
    action: &str,
) -> Activity {
    let mut actor = ticket.user.clone().unwrap_or_else(|| owner(repo));
    if matches!(action, "merged" | "closed") {
        let detail = if prefix == "pr" {
            app.github.pull(repo, ticket.number).await.ok()
        } else {
            app.github.issue(repo, ticket.number).await.ok()
        };
        if let Some(who) = detail.and_then(|d| d.merged_by.or(d.closed_by)) {
            actor = who;
        }
    }
    let what = if prefix == "pr" { "" } else { " issue" };
    let color = match action {
        "merged" => MERGED,
        "closed" if prefix == "pr" => ui::RED,
        "closed" => MERGED,
        _ => ui::GREEN,
    };
    Activity {
        kind: if prefix == "pr" {
            "pull_request"
        } else {
            "issues"
        },
        repo: repo.to_owned(),
        title: format!(
            "{} {action}{what} **#{}**",
            actor_link(&actor),
            ticket.number
        ),
        actor,
        details: Some(format!(
            "> {}",
            ui::truncate(&ui::escape(&ticket.title), 120)
        )),
        url: ticket.html_url.clone(),
        color,
    }
}

async fn check_activity(app: &App) {
    let servers = servers(app);
    let repos: BTreeSet<String> = servers
        .iter()
        .filter(|(_, c)| c.github.enabled && c.github.channel.is_some())
        .flat_map(|(_, c)| c.github.repos.iter().cloned())
        .collect();
    for repo in repos {
        let activities = match poll(app, &repo).await {
            Ok(activities) => activities,
            Err(error) => {
                log_error(&format!("GitHub activity of {repo}"), &error);
                continue;
            }
        };
        for activity in activities {
            let card = card(&activity);
            for (guild, config) in &servers {
                let feed = &config.github;
                let msg = match dc_core::guild::color_value(&feed.color) {
                    Some(color) => card.clone().recolor(color),
                    None => card.clone(),
                };
                let Some(channel) = feed.channel.as_deref() else {
                    continue;
                };
                let follows = feed.enabled
                    && feed.repos.contains(&repo)
                    && feed.events.iter().any(|event| event == activity.kind);
                if follows && let Err(error) = app.api.send_message(channel, &msg).await {
                    tracing::warn!(guild, %error, "posting GitHub activity failed");
                }
            }
        }
    }
}

pub async fn watch_activity(app: Arc<App>) {
    loop {
        tokio::time::sleep(pace(&app, ACTIVITY_EVERY, 900)).await;
        check_activity(&app).await;
    }
}

/// A sample card for the dashboard's test: the latest commit of the first
/// repository.
pub async fn sample(app: &App, repo: &str) -> Result<Msg, String> {
    let info = app.github.repo(repo).await.map_err(|e| e.to_string())?;
    let branch = if info.default_branch.is_empty() {
        "main".to_owned()
    } else {
        info.default_branch
    };
    let commits = app
        .github
        .commits(repo, &branch)
        .await
        .map_err(|e| e.to_string())?;
    let head = commits.first().ok_or("This repository has no commits.")?;
    let before = commits.get(1).map_or(head.sha.as_str(), |c| c.sha.as_str());
    Ok(card(&push_activity(
        repo,
        &branch,
        before,
        std::slice::from_ref(head),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::Asset;

    fn release(prerelease: bool) -> Release {
        Release {
            id: 1,
            tag_name: "0.3.4".into(),
            body: Some(format!(
                "## What's new\n- {}\n#### Detail",
                "x".repeat(3000)
            )),
            html_url: "https://github.com/Misty-MC/Misty/releases/tag/0.3.4".into(),
            published_at: Some("2026-11-08T10:00:00Z".into()),
            prerelease,
            draft: false,
            assets: vec![Asset {
                name: "Misty-Setup-0.3.4.exe".into(),
                size: 9_437_184,
                download_count: 120,
            }],
        }
    }

    #[test]
    fn release_cards() {
        let settings = Releases {
            role: Some("42".into()),
            download_url: "https://misty-launcher.online/download/setup".into(),
            ..Releases::default()
        };
        let msg = release_card(&settings, "Misty-MC/Misty", &release(false), true);
        let text = msg.payload().to_string();
        assert!(text.contains("<@&42>") && text.contains("### What's new"));
        // The mention comes before the card, like on the Misty bot.
        assert_eq!(msg.payload()["components"][0]["type"], 10);
        assert_eq!(msg.payload()["components"][1]["accent_color"], 0x6F9CFF);
        assert!(msg.fingerprint().contains("Nov 8, 2026, 10:00 UTC"));
        assert!(text.contains("9.0 MB") && text.contains("download/setup"));
        assert_eq!(msg.payload()["allowed_mentions"]["roles"][0], "42");
        let quiet = release_card(&settings, "Misty-MC/Misty", &release(true), false);
        assert!(!quiet.payload().to_string().contains("<@&42>"));
        let download = download_card(&settings, "Misty-MC/Misty", Some(&release(false)));
        assert!(download.payload().to_string().contains("**Downloads** 120"));
        assert!(
            download_card(&settings, "Misty-MC/Misty", None)
                .payload()
                .to_string()
                .contains("cannot be reached")
        );
    }

    #[test]
    fn push_cards_list_commits() {
        let commit = |sha: &str, message: &str| github::Commit {
            sha: sha.into(),
            commit: github::CommitDetail {
                message: message.into(),
                author: None,
            },
            author: None,
        };
        let fresh = vec![
            commit("2222222bbbb", "Second `code`\n\ndetail"),
            commit("1111111aaaa", "First"),
        ];
        let activity = push_activity("Misty-MC/Misty", "main", "0000000", &fresh);
        let text = card(&activity).payload().to_string();
        assert!(text.contains("**2** commits") && text.contains("2222222"));
        assert!(!text.contains("detail") && !text.contains("`code`"));
        assert_eq!(activity.actor.login, "Misty-MC");
    }
}
