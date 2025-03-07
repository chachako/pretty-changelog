/// Command-line argument parser.
pub mod args;
/// Changelog generator.
pub mod changelog;

#[macro_use]
extern crate log;

use args::{
	Opt,
	Sort,
	Strip,
};
use changelog::Changelog;
use clap::ArgEnum;
use git_cliff_core::commit::Commit;
use git_cliff_core::config::Config;
use git_cliff_core::embed::EmbeddedConfig;
use git_cliff_core::error::{
	Error,
	Result,
};
use git_cliff_core::release::Release;
use git_cliff_core::repo::Repository;
use git_cliff_core::DEFAULT_CONFIG;
use std::env;
use std::fs::{
	self,
	File,
};
use std::io;

/// Checks for a new version on crates.io
#[cfg(feature = "update-informer")]
fn check_new_version() {
	use update_informer::Check;
	let pkg_name = env!("CARGO_PKG_NAME");
	let pkg_version = env!("CARGO_PKG_VERSION");
	let informer = update_informer::new(
		update_informer::registry::Crates,
		pkg_name,
		pkg_version,
	);
	if let Some(new_version) = informer.check_version().ok().flatten() {
		log::info!(
			"A new version of {pkg_name} is available: v{pkg_version} -> \
			 {new_version}",
		);
	}
}

/// Runs `git-cliff`.
pub async fn run(mut args: Opt) -> Result<()> {
	// Check if there is a new version available.
	#[cfg(feature = "update-informer")]
	check_new_version();

	// Create the configuration file if init flag is given.
	if args.init {
		info!("Saving the configuration file to {:?}", DEFAULT_CONFIG);
		fs::write(DEFAULT_CONFIG, EmbeddedConfig::get_config()?)?;
		return Ok(());
	}

	// Set the working directory.
	if let Some(ref workdir) = args.workdir {
		args.config = workdir.join(args.config);
		args.repository = match args.repository {
			Some(repository) => Some(workdir.join(repository)),
			None => Some(workdir.clone()),
		};
		if let Some(changelog) = args.prepend {
			args.prepend = Some(workdir.join(changelog));
		}
	}

	// Parse the configuration file.
	let mut path = args.config.clone();
	if !path.exists() {
		if let Some(config_path) = dirs_next::config_dir()
			.map(|dir| dir.join(env!("CARGO_PKG_NAME")).join(DEFAULT_CONFIG))
		{
			path = config_path;
		}
	}

	// Load the default configuration if necessary.
	let mut config = if path.exists() {
		Config::parse(&path)?
	} else {
		if !args.context {
			warn!(
				"{:?} is not found, using the default configuration.",
				args.config
			);
		}
		EmbeddedConfig::parse()?
	};
	if config.changelog.body.is_none() && !args.context {
		warn!("Changelog body is not specified, using the default template.");
		config.changelog.body = EmbeddedConfig::parse()?.changelog.body;
	}

	// Update the configuration based on command line arguments and vice versa.
	match args.strip {
		Some(Strip::Header) => {
			config.changelog.header = None;
		}
		Some(Strip::Footer) => {
			config.changelog.footer = None;
		}
		Some(Strip::All) => {
			config.changelog.header = None;
			config.changelog.footer = None;
		}
		None => {}
	}
	if args.prepend.is_some() {
		config.changelog.footer = None;
		if !(args.unreleased || args.latest || args.range.is_some()) {
			return Err(Error::ArgumentError(String::from(
				"'-u' or '-l' is not specified",
			)));
		}
	}
	if args.body.is_some() {
		config.changelog.body = args.body.clone();
	}
	if args.sort == Sort::Oldest {
		if let Some(ref sort_commits) = config.git.sort_commits {
			args.sort = Sort::from_str(sort_commits, true)
				.expect("Incorrect config value for 'sort_commits'");
		}
	}
	if !args.date_order {
		if let Some(date_order) = config.git.date_order {
			args.date_order = date_order;
		}
	}

	// Initialize the git repository.
	let repository =
		Repository::init(args.repository.clone().unwrap_or(env::current_dir()?))?;

	// Parse tags.
	let mut tags = repository.tags(&config.git.tag_pattern, args.date_order)?;

	// Skip tags.
	config.git.skip_tags = config.git.skip_tags.filter(|r| !r.as_str().is_empty());
	let skip_regex = config.git.skip_tags.as_ref();
	let ignore_regex = config.git.ignore_tags.as_ref();
	tags = tags
		.into_iter()
		.filter(|(_, name)| {
			// Keep skip tags to drop commits in the later stage.
			let skip = skip_regex.map(|r| r.is_match(name)).unwrap_or_default();

			let ignore = ignore_regex
				.map(|r| {
					if r.as_str().trim().is_empty() {
						return false;
					}

					let ignore_tag = r.is_match(name);
					if ignore_tag {
						trace!("Ignoring release: {}", name)
					}
					ignore_tag
				})
				.unwrap_or_default();

			skip || !ignore
		})
		.collect();

	// Print debug information about configuration and arguments.
	log::trace!("{:#?}", args);
	log::trace!("{:#?}", config);

	// Parse commits.
	let mut commit_range = args.range;
	if args.unreleased {
		if let Some(last_tag) = tags.last().map(|(k, _)| k) {
			commit_range = Some(format!("{}..HEAD", last_tag));
		}
	} else if args.latest || args.current {
		if tags.len() < 2 {
			let commits = repository.commits(None, None, None)?;
			if let (Some(tag1), Some(tag2)) = (
				commits.last().map(|c| c.id().to_string()),
				tags.get_index(0).map(|(k, _)| k),
			) {
				commit_range = Some(format!("{}..{}", tag1, tag2));
			}
		} else {
			let mut tag_index = tags.len() - 2;
			if args.current {
				if let Some(current_tag_index) =
					repository.current_tag().as_ref().and_then(|tag| {
						tags.iter()
							.enumerate()
							.find(|(_, (_, v))| v == &tag)
							.map(|(i, _)| i)
					}) {
					tag_index = current_tag_index - 1;
				} else {
					return Err(Error::ChangelogError(String::from(
						"No tag exists for the current commit",
					)));
				}
			}
			if let (Some(tag1), Some(tag2)) = (
				tags.get_index(tag_index).map(|(k, _)| k),
				tags.get_index(tag_index + 1).map(|(k, _)| k),
			) {
				commit_range = Some(format!("{}..{}", tag1, tag2));
			}
		}
	}
	let mut commits =
		repository.commits(commit_range, args.include_path, args.exclude_path)?;
	if let Some(commit_limit_value) = config.git.limit_commits {
		commits = commits.drain(..commit_limit_value).collect();
	}

	// Update tags.
	if let Some(tag) = args.tag {
		if let Some(commit_id) = commits.first().map(|c| c.id().to_string()) {
			match tags.get(&commit_id) {
				Some(tag) => {
					warn!("There is already a tag ({}) for {}", tag, commit_id)
				}
				None => {
					tags.insert(commit_id, tag);
				}
			}
		}
	}

	// Process releases.
	let mut releases = vec![Release::default()];
	let mut release_index = 0;
	let mut previous_release = Release::default();
	for git_commit in commits.into_iter().rev() {
		let commit = Commit::from(&git_commit);
		let commit_id = commit.id.to_string();
		if args.sort == Sort::Newest {
			releases[release_index].commits.insert(0, commit);
		} else {
			releases[release_index].commits.push(commit);
		}
		if let Some(tag) = tags.get(&commit_id) {
			releases[release_index].version = Some(tag.to_string());
			releases[release_index].commit_id = Some(commit_id);
			releases[release_index].timestamp = git_commit.time().seconds();
			previous_release.previous = None;
			releases[release_index].previous = Some(Box::new(previous_release));
			previous_release = releases[release_index].clone();
			releases.push(Release::default());
			release_index += 1;
		}
	}

	// Add custom commit messages to the latest release.
	if let Some(custom_commits) = args.with_commit {
		if let Some(latest_release) = releases.iter_mut().last() {
			custom_commits.into_iter().for_each(|message| {
				latest_release.commits.push(Commit::from(message))
			});
		}
	}

	// Set the previous release if needed.
	if let Some((commit_id, version)) = tags.len().checked_sub(2).and_then(|v| tags.get_index(v)) {
		let previous_release = Release {
			commit_id: Some(commit_id.to_string()),
			version: Some(version.to_string()),
			..Release::default()
		};
		releases[0].previous = Some(Box::new(previous_release));
	}

	// Generate changelog.
	let changelog = Changelog::new(
		releases,
		&config,
		repository.remote_urls().ok(),
		args.github_token.clone(),
	).await?;
	if let Some(path) = args.prepend {
		changelog.prepend(fs::read_to_string(&path)?, &mut File::create(path)?)
	} else if let Some(path) = args.output {
		let mut output = File::create(path)?;
		if args.context {
			changelog.write_context(&mut output)
		} else {
			changelog.generate(&mut output)
		}
	} else if args.context {
		changelog.write_context(&mut io::stdout())
	} else {
		changelog.generate(&mut io::stdout())
	}
}
