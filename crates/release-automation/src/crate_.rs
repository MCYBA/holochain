use anyhow::{bail, Context};
use bstr::ByteSlice;
use cargo::util::VersionExt;
use log::{debug, info, warn};
use semver::Version;
use std::collections::{HashMap, HashSet};
use structopt::StructOpt;

use crate::{
    crate_selection::Crate,
    release::{crates_index_helper, ReleaseWorkspace},
    CommandResult, Fallible,
};

#[derive(StructOpt, Debug)]
pub(crate) struct CrateArgs {
    #[structopt(subcommand)]
    pub(crate) command: CrateCommands,
}

#[derive(Debug, StructOpt)]
pub(crate) struct CrateSetVersionArgs {
    #[structopt(long)]
    pub(crate) crate_name: String,

    #[structopt(long)]
    pub(crate) new_version: Version,
}

pub(crate) static DEFAULT_DEV_SUFFIX: &str = "dev.0";

#[derive(Debug, StructOpt)]
pub(crate) struct CrateApplyDevVersionsArgs {
    #[structopt(long, default_value = DEFAULT_DEV_SUFFIX)]
    pub(crate) dev_suffix: String,

    #[structopt(long)]
    pub(crate) dry_run: bool,

    #[structopt(long)]
    pub(crate) commit: bool,

    #[structopt(long)]
    pub(crate) no_verify: bool,
}

#[derive(Debug)]
pub(crate) enum FixupReleases {
    Latest,
    All,
    Selected(Vec<String>),
}

/// Parses an input string to an ordered set of release steps.
pub(crate) fn parse_fixup_releases(input: &str) -> Fallible<FixupReleases> {
    use std::str::FromStr;

    let words = input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect::<Vec<String>>();

    if let Some(first) = words.first() {
        match first.as_str() {
            "latest" => return Ok(FixupReleases::Latest),
            "all" => return Ok(FixupReleases::All),
            _ => {}
        }
    }

    Ok(FixupReleases::Selected(words))
}

#[derive(Debug, StructOpt)]
pub(crate) struct CrateFixupReleases {
    #[structopt(long, default_value = DEFAULT_DEV_SUFFIX)]
    pub(crate) dev_suffix: String,

    #[structopt(long)]
    pub(crate) dry_run: bool,

    #[structopt(long, default_value = "latest", parse(try_from_str = parse_fixup_releases))]
    pub(crate) fixup_releases: FixupReleases,

    #[structopt(long)]
    pub(crate) commit: bool,

    #[structopt(long)]
    pub(crate) no_verify: bool,
}

#[derive(Debug, StructOpt)]
pub(crate) struct CrateCheckArgs {
    #[structopt(long)]
    offline: bool,
}

pub(crate) const MINIMUM_CRATE_OWNERS: &str =
    "github:holochain:core-dev,holochain-release-automation,holochain-release-automation2,zippy,steveej";

#[derive(Debug, StructOpt)]
pub(crate) struct EnsureCrateOwnersArgs {
    #[structopt(long)]
    dry_run: bool,

    /// Assumes the default crate owners that are ensured to be set for each crate in the workspace.
    #[structopt(
        long,
        default_value = MINIMUM_CRATE_OWNERS,
        use_delimiter = true,
        multiple = false,

    )]
    minimum_crate_owners: Vec<String>,
}

#[derive(Debug, StructOpt)]
pub(crate) enum CrateCommands {
    SetVersion(CrateSetVersionArgs),
    ApplyDevVersions(CrateApplyDevVersionsArgs),

    /// check the latest (or given) release for crates that aren't published, remove their tags, and bump their version.
    FixupReleases(CrateFixupReleases),

    Check(CrateCheckArgs),
    EnsureCrateOwners(EnsureCrateOwnersArgs),
}

pub(crate) fn cmd(args: &crate::cli::Args, cmd_args: &CrateArgs) -> CommandResult {
    let ws = ReleaseWorkspace::try_new(args.workspace_path.clone())?;

    match &cmd_args.command {
        CrateCommands::SetVersion(subcmd_args) => {
            let crt = *ws
                .members()?
                .iter()
                .find(|crt| crt.name() == subcmd_args.crate_name)
                .ok_or_else(|| anyhow::anyhow!("crate {} not found", subcmd_args.crate_name))?;

            crate::common::set_version(false, crt, &subcmd_args.new_version)?;

            Ok(())
        }

        CrateCommands::ApplyDevVersions(subcmd_args) => apply_dev_versions(
            &ws,
            &subcmd_args.dev_suffix,
            subcmd_args.dry_run,
            subcmd_args.commit,
            subcmd_args.no_verify,
        ),

        CrateCommands::FixupReleases(subcmd_args) => fixup_releases(
            &ws,
            &subcmd_args.dev_suffix,
            &subcmd_args.fixup_releases,
            subcmd_args.dry_run,
            subcmd_args.commit,
            subcmd_args.no_verify,
        ),

        CrateCommands::Check(subcmd_args) => {
            ws.cargo_check(subcmd_args.offline, std::iter::empty::<&str>())?;

            Ok(())
        }
        CrateCommands::EnsureCrateOwners(subcmd_args) => {
            ensure_crate_io_owners(
                &ws,
                subcmd_args.dry_run,
                ws.members()?,
                subcmd_args
                    .minimum_crate_owners
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .as_slice(),
            )?;

            Ok(())
        }
    }
}

/// Scans the workspace for crates that have changed since their previous release and bumps their version to a dev version.
///
/// This is a crucial part of the release flow to prevent inconsistencies in publishing dependents of these changed crates.
/// For example:
/// crate A is being published and depends on crate B and its changes since its last release.
/// crate B however hasn't increased its version number since the last release, so it looks as if the most recent version is already published.
/// This causes crate A to be published with a dependency on version of crate B that doesn't contain the changes that crate A depends upon.
/// Hence the newly published version of crate A is broken.
/// To prevent this, we increase crate B's version to a develop version that hasn't been published yet.
/// This will detect a missing dependency in an attempt to publish crate A, as the dev version of crate B is not found on the registry.
/// Note that we wouldn't publish the develop version of crate B, as the regular workspace release flow also increases its version according to the configured scheme.
pub(crate) fn apply_dev_versions<'a>(
    ws: &'a ReleaseWorkspace<'a>,
    dev_suffix: &str,
    dry_run: bool,
    commit: bool,
    no_verify: bool,
) -> Fallible<()> {
    let applicable_crates = ws
        .members()?
        .iter()
        .filter(|crt| crt.state().changed_since_previous_release())
        .cloned()
        .collect::<Vec<_>>();

    let msg = apply_dev_vesrions_to_selection(applicable_crates, dev_suffix, dry_run)?;

    if !msg.is_empty() {
        let commit_msg = indoc::formatdoc! {r#"
            apply develop versions to changed crates

            the following crates changed since their most recent release
            and are therefore increased to a develop version:
            {}
        "#, msg,
        };

        info!("creating commit with message '{}' ", commit_msg);

        if !dry_run {
            // this checks consistency and also updates the Cargo.lock file(s)
            if !no_verify {
                ws.cargo_check(false, std::iter::empty::<&str>())?;
            }

            if commit {
                ws.git_add_all_and_commit(&commit_msg, None)?;
            }
        }
    }

    Ok(())
}

pub(crate) fn apply_dev_vesrions_to_selection<'a>(
    applicable_crates: Vec<&'a Crate<'a>>,
    dev_suffix: &str,
    dry_run: bool,
) -> Fallible<String> {
    let mut applicable_crates = applicable_crates
        .iter()
        .map(|crt| (crt.name(), *crt))
        .collect::<HashMap<_, _>>();

    let mut queue = applicable_crates.values().copied().collect::<Vec<_>>();
    let mut msg = String::new();

    while let Some(crt) = queue.pop() {
        let mut version = crt.version();

        if version.is_prerelease() {
            debug!(
                "[{}] ignoring due to prerelease version '{}' after supposed release",
                crt.name(),
                version,
            );

            continue;
        }

        increment_patch(&mut version);
        version = semver::Version::parse(&format!("{}-{}", version, dev_suffix))?;

        debug!(
            "[{}] rewriting version {} -> {}",
            crt.name(),
            crt.version(),
            version,
        );

        for changed_dependant in crate::common::set_version(dry_run, crt, &version)? {
            if applicable_crates
                .insert(changed_dependant.name(), changed_dependant)
                .is_none()
                && changed_dependant.state().has_previous_release()
            {
                queue.push(changed_dependant);
            }
        }

        // todo: can we mutate crt and use crt.name_version() here instead?
        msg += format!("\n- {}-{}", crt.name(), version).as_str();
    }

    Ok(msg)
}

pub(crate) fn increment_patch(v: &mut semver::Version) {
    v.patch += 1;
    v.pre = semver::Prerelease::EMPTY;
    v.build = semver::BuildMetadata::EMPTY;
}

pub(crate) fn fixup_releases<'a>(
    ws: &'a ReleaseWorkspace<'a>,
    dev_suffix: &str,
    fixup: &FixupReleases,
    dry_run: bool,
    commit: bool,
    no_verify: bool,
) -> Fallible<()> {
    let mut unpublished_crates: std::collections::BTreeMap<
        String,
        Vec<&'a crate::crate_selection::Crate>,
    > = Default::default();

    match fixup {
        FixupReleases::Latest => {
            let (release_title, crate_release_titles) = match ws
                .changelog()
                .map(|cl| cl.topmost_release())
                .transpose()?
                .flatten()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no topmost release found in changelog '{:?}'. nothing to publish",
                        ws.changelog()
                    )
                })? {
                crate::changelog::ReleaseChange::WorkspaceReleaseChange(title, releases) => (
                    title,
                    releases
                        .into_iter()
                        .collect::<std::collections::BTreeSet<_>>(),
                ),
                unexpected => bail!("unexpected topmost release: {:?}", unexpected),
            };

            debug!("{}: {:#?}", release_title, crate_release_titles);

            let crates = ws
                .members()?
                .iter()
                .filter(|crt| crate_release_titles.contains(&crt.name_version()))
                .cloned()
                .collect::<Vec<_>>();

            for crt in crates {
                if !crate::release::crates_index_helper::is_version_published(crt, false)? {
                    unpublished_crates
                        .entry(release_title.clone())
                        .or_default()
                        .push(crt);
                }
            }
        }
        other => bail!("{:?} not implemented", other),
    }

    info!(
        "the following crates are unpublished: {:#?}",
        unpublished_crates
            .iter()
            .map(|(release, crts)| (
                release,
                crts.iter()
                    .map(|crt| crt.name_version())
                    .collect::<Vec<_>>()
            ))
            .collect::<Vec<_>>()
    );

    // bump their versions to dev versions
    let msg = apply_dev_vesrions_to_selection(
        // TOOD: change this once more than "latest" is supported above
        unpublished_crates.into_iter().next().unwrap_or_default().1,
        dev_suffix,
        dry_run,
    )?;

    if !msg.is_empty() {
        let commit_msg = indoc::formatdoc! {r#"
            applying develop versions to unpublished crates

            bumping the following crates to their dev versions to retrigger the release process for the failed crates
            {}
        "#, msg,
        };

        info!("creating commit with message '{}' ", commit_msg);

        if !dry_run {
            // this checks consistency and also updates the Cargo.lock file(s)
            if !no_verify {
                ws.cargo_check(false, std::iter::empty::<&str>())?;
            };

            if commit {
                ws.git_add_all_and_commit(&commit_msg, None)?;
            }
        }
    }

    Ok(())
}

/// Ensures that the given crates have at least sent an invite to the given crate.io usernames.
pub(crate) fn ensure_crate_io_owners<'a>(
    _ws: &'a ReleaseWorkspace<'a>,
    dry_run: bool,
    crates: &[&Crate],
    minimum_crate_owners: &[&str],
) -> Fallible<()> {
    let desired_owners = minimum_crate_owners
        .iter()
        .map(|s| s.to_string())
        .collect::<HashSet<_>>();

    for crt in crates {
        if !crates_index_helper::is_version_published(crt, false)? {
            warn!("{} is not published, skipping..", crt.name());
            continue;
        }

        let mut cmd = std::process::Command::new("cargo");
        cmd.args(&["owner", "--list", &crt.name()]);

        debug!("[{}] running command: {:?}", crt.name(), cmd);
        let output = cmd.output().context("process exitted unsuccessfully")?;
        if !output.status.success() {
            warn!(
                "[{}] failed list owners: {}",
                crt.name(),
                String::from_utf8_lossy(&output.stderr)
            );

            continue;
        }

        let current_owners = output
            .stdout
            .lines()
            .map(|line| {
                line.words_with_breaks()
                    .take_while(|item| *item != " ")
                    .collect::<String>()
            })
            .collect::<HashSet<_>>();
        let diff = desired_owners.difference(&current_owners);
        info!(
            "[{}] current owners {:?}, missing owners: {:?}",
            crt.name(),
            current_owners,
            diff
        );

        for owner in diff {
            let mut cmd = std::process::Command::new("cargo");
            cmd.args(&["owner", "--add", owner, &crt.name()]);

            debug!("[{}] running command: {:?}", crt.name(), cmd);
            if !dry_run {
                let output = cmd.output().context("process exitted unsuccessfully")?;
                if !output.status.success() {
                    warn!(
                        "[{}] failed to add owner '{}': {}",
                        crt.name(),
                        owner,
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
            }
        }
    }

    Ok(())
}
