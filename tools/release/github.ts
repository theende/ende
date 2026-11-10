// Copyright 2018-2026 the Deno authors. MIT license.

// The GitHub API requests the release scripts make. They used npm:octokit
// through jsr:@deno/rust-automation, which Ende can't load, and only ever
// POST a JSON body. This sends the same `accept`, `authorization` and
// `content-type` headers octokit did. Unlike octokit it never retries: a
// non-2xx response throws, with the response body.

const API_URL = "https://api.github.com";

/** The owner and repo in GITHUB_REPOSITORY, which GitHub Actions sets. */
export function getGitHubRepository(): { owner: string; repo: string } {
  const repoEnvVar = getEnvVarOrThrow("GITHUB_REPOSITORY");
  const [owner, repo] = repoEnvVar.split("/");
  if (repo === undefined) {
    throw new Error(
      `Environment variable GITHUB_REPOSITORY ` +
        `must be formatted as "{owner}/{repo}".`,
    );
  }
  return {
    owner,
    repo,
  };
}

export class GitHubClient {
  readonly #token: string;

  constructor(token: string) {
    this.#token = token;
  }

  /** POSTs `body` as JSON to the API path and returns the response's JSON. */
  async post(path: string, body: unknown): Promise<Record<string, unknown>> {
    const response = await fetch(`${API_URL}${path}`, {
      method: "POST",
      headers: {
        "accept": "application/vnd.github.v3+json",
        "authorization": `token ${this.#token}`,
        "content-type": "application/json; charset=utf-8",
        "user-agent": "ende-release-scripts",
      },
      body: JSON.stringify(body),
    });
    const text = await response.text();
    if (!response.ok) {
      throw new Error(
        `POST ${path} failed: ${response.status} ${response.statusText}\n${text}`,
      );
    }
    return JSON.parse(text);
  }
}

/** A client that authenticates with GITHUB_TOKEN. */
export function createGitHubClient(): GitHubClient {
  return new GitHubClient(getEnvVarOrThrow("GITHUB_TOKEN"));
}

function getEnvVarOrThrow(name: string) {
  const value = Deno.env.get(name);
  if (value == null) {
    throw new Error(
      `Could not find environment variable ${name}. ` +
        `Ensure you are running in a GitHub action.`,
    );
  }
  return value;
}
