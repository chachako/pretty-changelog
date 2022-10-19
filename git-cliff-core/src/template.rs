use crate::error::{
	Error,
	Result,
};
use crate::release::Release;
use std::collections::{BTreeMap, HashMap};
use std::error::Error as ErrorImpl;
use std::fmt::Write;
use std::thread::scope;
use regex::Regex;
use tera::{
	Context as TeraContext,
	Result as TeraResult,
	Tera,
	Value,
};

/// Wrapper for [`Tera`].
#[derive(Debug)]
pub struct Template {
	tera: Tera,
}

impl Template {
	/// Constructs a new instance.
	pub fn new(template: String) -> Result<Self> {
		let mut tera = Tera::default();
		if let Err(e) = tera.add_raw_template("template", &template) {
			return if let Some(error_source) = e.source() {
				Err(Error::TemplateParseError(error_source.to_string()))
			} else {
				Err(Error::TemplateError(e))
			};
		}
		tera.register_filter("upper_first", Self::upper_first_filter);
		Ok(Self { tera })
	}

	fn upper_first(value: &str) -> String {
		let mut c = value.chars();
		match c.next() {
			None => String::new(),
			Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
		}
	}

	/// Filter for making the first character of a string uppercase.
	fn upper_first_filter(
		value: &Value,
		_: &HashMap<String, Value>,
	) -> TeraResult<Value> {
		let mut s =
			tera::try_get_value!("upper_first_filter", "value", String, value);
		s = Self::upper_first(&s);
		Ok(tera::to_value(&s)?)
	}

	/// Renders the template.
	pub fn render(&self, release: &Release) -> Result<String> {
		let context = TeraContext::from_serialize(release)?;
		match self.tera.render("template", &context) {
			Ok(v) => Ok(v),
			Err(e) => {
				return if let Some(error_source) = e.source() {
					Err(Error::TemplateRenderError(error_source.to_string()))
				} else {
					Err(Error::TemplateError(e))
				};
			}
		}
	}

	/// Renders default template.
	pub fn render_default(release: &Release, github_repo: Option<String>) -> Result<String> {
		let repo_owner = &github_repo
			.clone()
			.map(|repo| repo.split('/').next().unwrap().to_string());
		let repo_url = &github_repo.map(|repo| format!("https://github.com/{repo}"));
		let mut result = String::new();
		if let Some(version) = &release.version {
			// ## [0.1.0] - 2222-22-22
			writeln!(
				result,
				"## [{}] - {}\n",
				version.trim_start_matches('v'),
				chrono::NaiveDateTime::from_timestamp(release.timestamp, 0)
					.format("%Y-%m-%d")
			)
		} else {
			writeln!(result, "## [Unreleased]\n")
		}?;

		// Groups { Scopes { Commits[] }, ... }
		let mut grouped = BTreeMap::new();
		for commit in &release.commits {
			// Map only the commit with group, because it follows "conventional commits"
			let group = commit.group.clone();
			let conv_group = commit.conv.clone().map(|c| c.type_().to_string());
			if let Some(group) = group.or(conv_group) {
				let scope = commit
					.scope
					.as_deref()
					.or_else(||
						commit.conv
							.as_ref()
							.and_then(|c| c.scope())
							.map(|s| s.as_str())
					)
					.or(commit.default_scope.as_deref());
				// Group by scope
				grouped
					.entry(group)
					.or_insert_with(BTreeMap::new)
					.entry(scope)
					.or_insert_with(Vec::new)
					.push(commit);
			}
		}

		for (group, scopes) in grouped {
			// ## Group
			writeln!(result, "### {}", group
				.trim_start_matches(|c: char| c.is_numeric())
				.trim_start_matches(". "))?;

			for (scope, commits) in scopes {
				// #### - Scope, OtherScope
				if let Some(scope) = scope {
					let scope = scope
						.split(',')
						.map(|s| Self::upper_first(s.trim()))
						.collect::<Vec<_>>()
						.join(", ");

					writeln!(result, "\n#### - {scope}\n")?;
				}
				for commit in commits {
					let authors = commit.github_authors();
					let prs = commit.pull_requests();
					let mut message = Self::upper_first(
						commit.conv
							.as_ref()
							.map(|c| c.description())
							.unwrap_or(&commit.message)
					);

					if !authors.is_empty() &&
						// Skip if only owner
						!(authors.len() == 1 && authors.first().cloned() == repo_owner.clone()) {
						// Commit message by [@author1](link) and [@author2](link)
						message = format!(
							"{} by {}",
							message,
							authors.iter()
								.map(|author| format!("[@{author}](https://github.com/{author})"))
								.collect::<Vec<String>>()
								.join(" and ")
						)
					}

					if !prs.is_empty() && repo_url.is_some() {
						// Commit message.. in [#1](link) and [#2](link)
						message = format!(
							"{} in {}",
							message,
							prs.iter()
								.map(|pr| format!("[#{pr}]({}/pull/{pr})", repo_url.as_ref().unwrap()))
								.collect::<Vec<String>>()
								.join(" and ")
						)
					}

					// - [`short_hash`](link) Commit message
					let short_hash = &commit.id[0..7];
					if let Some(repo) = &repo_url {
						writeln!(
							result,
							"- [`{short_hash}`]({repo}/commit/{}) {message}",
							commit.id,
						)?;
					} else {
						writeln!(result, "- `{short_hash}` {message}")?;
					}

					//   　
					//   > Commit body line1
					//   > Commit body line2
					if let Some(Some(body)) = commit.conv.as_ref().map(|c| c.body()) {
						// Skip Github squash messages
						let squash_msg_prefix = Regex::new(r"^\*[[:space:]]\w+").unwrap();
						if !body.is_empty() && !squash_msg_prefix.is_match(body) {
							writeln!(result, "  　")?;
							for line in body.lines() {
								writeln!(result, "  > {}", line)?;
							}
						}
					}
				}
			}

			writeln!(result, "\n---\n")?;
		}

		// _This changelog is generated by [git-cliff](https://github.com/orhun/git-cliff),_
		// _**You can also view the full changes: https://github.com/chachako/checkout-tags/compare/v1.0..v1.2**_
		write!(
			result,
			"_This changelog is generated by [pretty-changelog](https://github.com/chachako/pretty-changelog)"
		)?;
		if let Some(repo) = repo_url {
			writeln!(result, ",_")?;
			write!(result, "_**You can also view the full changes: {repo}/")?;
			if let Some(Some(prev)) = release.previous.as_ref().map(|v| v.version.clone()) {
				let current_version = release.version.as_deref().unwrap_or("HEAD");
				write!(result, "compare/{prev}..{current_version}")?;
			} else {
				write!(result, "commits/HEAD")?;
			}
			writeln!(result, "**_")?;
		}
		writeln!(result, "\n---\n")?;

		Ok(result)
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use crate::commit::Commit;

	#[test]
	fn render_template() -> Result<()> {
		let template = r#"
		## {{ version }}
		{% for commit in commits %}
		### {{ commit.group }}
		- {{ commit.message | upper_first }}
		{% endfor %}"#;
		let template = Template::new(template.to_string())?;
		assert_eq!(
			r#"
		## 1.0
		
		### feat
		- Add xyz
		
		### fix
		- Fix abc
		"#,
			template.render(&Release {
				version:   Some(String::from("1.0")),
				commits:   vec![
					Commit::new(
						String::from("123123"),
						String::from("feat(xyz): add xyz"),
					),
					Commit::new(
						String::from("124124"),
						String::from("fix(abc): fix abc"),
					)
				]
				.into_iter()
				.filter_map(|c| c.into_conventional().ok())
				.collect(),
				commit_id: None,
				timestamp: 0,
				previous:  None,
			})?
		);
		Ok(())
	}
}
