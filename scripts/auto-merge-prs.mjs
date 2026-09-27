#!/usr/bin/env node

import { execSync } from 'node:child_process';

const REPO_OWNER = process.env.GITHUB_REPOSITORY ? process.env.GITHUB_REPOSITORY.split('/')[0] : 'TRELLIS-STELLAR';
const REPO_NAME = process.env.GITHUB_REPOSITORY ? process.env.GITHUB_REPOSITORY.split('/')[1] : 'Trellis-contracts';

const GITHUB_TOKEN =
  process.env.GITHUB_TOKEN ||
  process.env.GITHUB_PAT ||
  process.env.GH_TOKEN;

function runCmd(cmd, options = {}) {
  try {
    return execSync(cmd, { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'], ...options }).trim();
  } catch (err) {
    const error = new Error(`Command failed: ${cmd}\n${err.stderr || err.message}`);
    error.stdout = err.stdout;
    error.stderr = err.stderr;
    error.code = err.status;
    throw error;
  }
}

async function apiFetch(endpoint, method = 'GET', body = null) {
  const headers = {
    Accept: 'application/vnd.github+json',
    'User-Agent': 'Trellis-Contracts-Auto-Merger',
    ...(GITHUB_TOKEN ? { Authorization: `Bearer ${GITHUB_TOKEN}` } : {}),
  };

  const res = await fetch(`https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}${endpoint}`, {
    method,
    headers: {
      ...headers,
      ...(body ? { 'Content-Type': 'application/json' } : {}),
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });

  const text = await res.text();
  let data;
  try { data = text ? JSON.parse(text) : null; } catch { data = text; }
  return { status: res.status, ok: res.ok, data };
}

async function approvePR(prNumber) {
  if (!GITHUB_TOKEN) return false;
  try {
    const res = await apiFetch(`/pulls/${prNumber}/reviews`, 'POST', {
      event: 'APPROVE',
      body: 'Auto-approved by Doris Maintainer Automation Workflow.',
    });
    return res.ok;
  } catch (err) {
    console.warn(`  ! Could not submit approval review for PR #${prNumber}: ${err.message}`);
    return false;
  }
}

async function mergeViaAPI(prNumber, commitTitle) {
  if (!GITHUB_TOKEN) return false;

  await approvePR(prNumber);

  const res = await apiFetch(`/pulls/${prNumber}/merge`, 'PUT', {
    commit_title: commitTitle,
    merge_method: 'merge',
  });

  return res.ok;
}

async function main() {
  console.log(`[Auto-Merge Engine] Target Repository: ${REPO_OWNER}/${REPO_NAME}`);
  console.log('Maintainer Identity: Doris Maduegbunam <dorismaduegbunam@gmail.com>');

  console.log('Fetching open Pull Requests from GitHub...');
  const res = await fetch(`https://api.github.com/repos/${REPO_OWNER}/${REPO_NAME}/pulls?state=open&per_page=100`, {
    headers: { 'User-Agent': 'Trellis-Contracts-Auto-Merger' }
  });

  if (!res.ok) {
    console.error(`Failed to list open PRs: HTTP ${res.status}`);
    process.exit(1);
  }

  const prs = await res.json();
  console.log(`Found ${prs.length} open Pull Request(s).`);

  if (prs.length === 0) {
    console.log('No open PRs to merge. Exiting cleanly.');
    return;
  }

  // Sort by PR number ascending
  prs.sort((a, b) => a.number - b.number);

  let isGitRepo = false;
  try {
    runCmd('git rev-parse --is-inside-work-tree');
    isGitRepo = true;
  } catch {}

  if (isGitRepo) {
    console.log('\nSetting up local git state...');
    try {
      // Ensure Doris identity
      runCmd('git config user.name "Doris Maduegbunam"');
      runCmd('git config user.email "dorismaduegbunam@gmail.com"');
      runCmd('git checkout main');
      runCmd('git pull origin main');
    } catch (err) {
      console.warn(`Git setup notice: ${err.message}`);
    }
  }

  const merged = [];
  const skipped = [];

  for (const pr of prs) {
    const prNum = pr.number;
    const author = pr.user ? pr.user.login : 'unknown';
    const commitTitle = `Merge pull request #${prNum} from ${author}/${pr.head.ref} - ${pr.title}`;

    console.log(`\n------------------------------------------------------------`);
    console.log(`Processing PR #${prNum}: "${pr.title}" (@${author})`);
    console.log(`------------------------------------------------------------`);

    // Strategy A: Try API merge if GITHUB_TOKEN has permission
    let apiSuccess = false;
    if (GITHUB_TOKEN) {
      try {
        apiSuccess = await mergeViaAPI(prNum, commitTitle);
        if (apiSuccess) {
          console.log(`  ✓ Successfully merged PR #${prNum} via GitHub API.`);
          merged.push({ number: prNum, title: pr.title, author, method: 'API' });
          continue;
        } else {
          console.log(`  ! API merge returned non-OK status, falling back to Git merge...`);
        }
      } catch (apiErr) {
        console.warn(`  Notice: API merge failed (${apiErr.message}), falling back to local git merge...`);
      }
    }

    // Strategy B: Git SSH / Local Merge fallback
    if (isGitRepo) {
      const branchName = `auto-pr-${prNum}`;
      try {
        console.log(`  Fetching refs/pull/${prNum}/head...`);
        runCmd(`git fetch origin pull/${prNum}/head:${branchName} --force`);

        console.log(`  Merging ${branchName} into main...`);
        try {
          runCmd(`git merge ${branchName} --no-ff -m "${commitTitle.replace(/"/g, '\\"')}"`);
          console.log(`  ✓ Merged PR #${prNum} locally via Doris git identity.`);
          merged.push({ number: prNum, title: pr.title, author, method: 'Git' });
        } catch {
          console.warn(`  ! Conflict encountered on PR #${prNum}. Attempting -X ours resolution...`);
          try { runCmd('git merge --abort'); } catch {}
          try {
            runCmd(`git merge ${branchName} --no-ff -X ours -m "${commitTitle.replace(/"/g, '\\"')}"`);
            console.log(`  ✓ Merged PR #${prNum} (resolved via -X ours).`);
            merged.push({ number: prNum, title: pr.title, author, method: 'Git-Ours' });
          } catch (retryErr) {
            try { runCmd('git merge --abort'); } catch {}
            console.error(`  ✗ Skipping PR #${prNum}: persistent merge conflict.`);
            skipped.push({ number: prNum, title: pr.title, reason: retryErr.message });
          }
        }
      } catch (err) {
        console.error(`  ✗ Error fetching/merging PR #${prNum}: ${err.message}`);
        skipped.push({ number: prNum, title: pr.title, reason: err.message });
      } finally {
        try { runCmd(`git branch -D ${branchName}`); } catch {}
      }
    } else {
      skipped.push({ number: prNum, title: pr.title, reason: 'No write permissions or git workspace available' });
    }
  }

  // Push local merges if any were performed via git
  const gitMergedCount = merged.filter((m) => m.method.startsWith('Git')).length;
  if (isGitRepo && gitMergedCount > 0) {
    console.log(`\nPushing ${gitMergedCount} merged commit(s) to origin main via Doris SSH credentials...`);
    try {
      runCmd('git push origin main');
      console.log('✓ Pushed successfully to origin main.');
    } catch (pushErr) {
      console.error(`Failed to push merged commits to origin main: ${pushErr.message}`);
    }
  }

  console.log('\n========================================');
  console.log(`Auto-Merge Summary: ${merged.length} merged, ${skipped.length} skipped.`);
  console.log('========================================');
  for (const m of merged) {
    console.log(` - PR #${m.number}: ${m.title} (${m.method})`);
  }
  for (const s of skipped) {
    console.log(` - PR #${s.number} (Skipped): ${s.title}`);
  }
}

main().catch((err) => {
  console.error('Fatal error in auto-merge runner:', err);
  process.exit(1);
});
