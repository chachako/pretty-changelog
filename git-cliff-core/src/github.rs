use reqwest::RequestBuilder;
use serde::Deserialize;
use crate::error::Result;

#[derive(Deserialize, Debug)]
struct Commit {
	author: Author,
}

#[derive(Deserialize, Debug)]
struct Author {
	login: String,
}

#[derive(Deserialize, Debug)]
pub struct Pr {
	number: u32,
}

pub async fn get_commit_author(
	token: &Option<String>,
	repo: &str,
	commit_sha: &str,
) -> Result<String> {
	let url = format!("https://api.github.com/repos/{repo}/commits/{commit_sha}");
	let commit = get_github(&url, token).send().await?.json::<Commit>().await?;
	Ok(commit.author.login)
}

pub async fn get_prs_associated_with_commit(
	token: &Option<String>,
	repo: &str,
	commit_sha: &str,
) -> Result<Vec<u32>> {
	let url = format!("https://api.github.com/repos/{repo}/commits/{commit_sha}/pulls");
	let prs = get_github(&url, token).send().await?.json::<Vec<Pr>>().await?;
	Ok(prs.into_iter().map(|p| p.number).collect())
}

pub async fn get_pr_authors(
	token: &Option<String>,
	repo: &str,
	pr_number: &u32,
) -> Result<Vec<String>> {
	let url = format!("https://api.github.com/repos/{repo}/pulls/{pr_number}/commits");
	let commits: Vec<Commit> = get_github(&url, token).send().await?.json().await?;
	let authors = commits.into_iter().map(|c| c.author.login).collect();
	Ok(authors)
}

fn get_github(url: &str, token: &Option<String>) -> RequestBuilder {
	let client = reqwest::Client::new();
	let mut request = client.get(url);
	if let Some(token) = token {
		request = request
			.header("Authorization", format!("token {token}"))
			.header("User-Agent", "git-cliff");
	}
	request
}