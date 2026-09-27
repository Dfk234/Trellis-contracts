import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

// Parse CLI args
const args = process.argv.slice(2);
let customFile = null;
let token = process.env.GITHUB_PAT || null;
const isDryRun = args.includes('--dry-run');

for (let i = 0; i < args.length; i++) {
  if (args[i] === '--file' && args[i + 1]) {
    customFile = args[i + 1];
    i++;
  } else if (!args[i].startsWith('--') && !token) {
    token = args[i];
  }
}

// Determine target issue file
const defaultFile = path.resolve(__dirname, '../.github/wave-2-issues.json');
const fallbackFile = path.resolve(__dirname, '../.github/wave-issues.json');
const issuesFilePath = customFile 
  ? path.resolve(process.cwd(), customFile)
  : (fs.existsSync(defaultFile) ? defaultFile : fallbackFile);

console.log(`Loading issues from: ${issuesFilePath}`);
const rawData = fs.readFileSync(issuesFilePath, 'utf8');
const issueData = JSON.parse(rawData);

const OWNER = 'TRELLIS-STELLAR';
const REPO = 'Trellis-contracts';

const LABEL_METADATA = {
  'legal': { color: '5319e7', description: 'Licensing, compliance, and legal considerations' },
  'ci': { color: 'fbca04', description: 'Continuous integration and automation workflows' },
  'community': { color: '006b75', description: 'Contributor onboarding, guidelines, and community health' },
  'security': { color: 'd93f0b', description: 'Security policies, vulnerabilities, and audits' },
  'core': { color: '1d76db', description: 'Core smart contract implementation and business logic' },
  'test': { color: 'bfdadc', description: 'Test coverage, harnesses, and test suites' },
  'refactor': { color: 'c5def5', description: 'Code cleanup, module reconciliation, and structural improvements' },
  'performance': { color: 'f9d0c4', description: 'Gas optimization, WASM size, and performance' },
  'needs discussion': { color: 'b60205', description: 'Requires architectural decision or maintainer consensus' },
  'governance': { color: '6f42c1', description: 'Governance proposals, multi-sig voting, and parameter adjustments' },
  'architecture': { color: '0052cc', description: 'Protocol system design, cross-contract calls, and storage layouts' },
  'deployment': { color: '1f883d', description: 'Deployment scripts, network verification, and setup automation' }
};

if (!token && !isDryRun) {
  console.error('Usage: node scripts/push-issues.js [GITHUB_PAT] [--file <path>] [--dry-run]');
  console.error('Or set GITHUB_PAT environment variable.');
  process.exit(1);
}

const headers = {
  'Accept': 'application/vnd.github+json',
  'User-Agent': 'Trellis-Issue-Automation',
  'X-GitHub-Api-Version': '2022-11-28'
};

if (token) {
  headers['Authorization'] = `Bearer ${token.trim()}`;
}

async function githubRequest(url, options = {}) {
  const fullUrl = url.startsWith('http') ? url : `https://api.github.com${url}`;
  const response = await fetch(fullUrl, {
    ...options,
    headers: {
      ...headers,
      ...(options.headers || {})
    }
  });

  const text = await response.text();
  let json;
  try {
    json = JSON.parse(text);
  } catch {
    json = text;
  }

  if (!response.ok) {
    const errorMsg = json && json.message ? json.message : text;
    throw new Error(`GitHub API Error (${response.status}): ${errorMsg}`);
  }

  return json;
}

async function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function fetchAllIssues() {
  const all = [];
  let page = 1;
  while (true) {
    const res = await githubRequest(`/repos/${OWNER}/${REPO}/issues?state=all&per_page=100&page=${page}`);
    if (!res || res.length === 0) break;
    all.push(...res);
    if (res.length < 100) break;
    page++;
  }
  return all;
}

async function main() {
  console.log(`Repository: ${OWNER}/${REPO}`);
  console.log(`Total issues to process: ${issueData.issues.length}`);
  if (isDryRun) {
    console.log('--- DRY RUN MODE (no changes will be made) ---\n');
  }

  // 1. Check existing issues
  let existingIssues = [];
  try {
    console.log('Fetching all existing repository issues for deduplication...');
    existingIssues = await fetchAllIssues();
    console.log(`Found ${existingIssues.length} existing issues & PRs.`);
  } catch (err) {
    console.error('Warning: could not fetch existing issues:', err.message);
  }

  const existingTitles = new Set(existingIssues.map((i) => i.title.trim().toLowerCase()));

  // 2. Ensure labels exist
  if (!isDryRun) {
    console.log('Checking / creating repository labels...');
    let existingLabels = [];
    try {
      existingLabels = await githubRequest(`/repos/${OWNER}/${REPO}/labels?per_page=100`);
    } catch (err) {
      console.error('Warning: could not fetch existing labels:', err.message);
    }

    const existingLabelNames = new Set(existingLabels.map((l) => l.name.toLowerCase()));

    // Collect all labels from the issues
    const allLabels = new Set();
    for (const issue of issueData.issues) {
      for (const label of issue.labels) {
        allLabels.add(label);
      }
    }

    for (const labelName of allLabels) {
      if (!existingLabelNames.has(labelName.toLowerCase())) {
        const meta = LABEL_METADATA[labelName] || { color: 'ededed', description: '' };
        console.log(`Creating label "${labelName}"...`);
        try {
          await githubRequest(`/repos/${OWNER}/${REPO}/labels`, {
            method: 'POST',
            body: JSON.stringify({
              name: labelName,
              color: meta.color,
              description: meta.description
            })
          });
          console.log(`  ✓ Label "${labelName}" created.`);
          await sleep(500);
        } catch (err) {
          console.error(`  ✗ Failed to create label "${labelName}":`, err.message);
        }
      }
    }
  }

  // 3. Create issues
  console.log('\nCreating issues...');
  let createdCount = 0;
  let skippedCount = 0;

  for (const issue of issueData.issues) {
    const { number, title, labels, body } = issue;
    console.log(`\n[#${number || 'New'}] "${title}"`);
    console.log(`  Labels: [${labels.join(', ')}]`);

    if (existingTitles.has(title.trim().toLowerCase())) {
      console.log(`  -> Skipped: issue with this title already exists.`);
      skippedCount++;
      continue;
    }

    if (isDryRun) {
      console.log(`  -> Would create issue with ${body.length} characters in body.`);
      createdCount++;
      continue;
    }

    try {
      const created = await githubRequest(`/repos/${OWNER}/${REPO}/issues`, {
        method: 'POST',
        body: JSON.stringify({
          title,
          body,
          labels
        })
      });

      console.log(`  ✓ Created: #${created.number} -> ${created.html_url}`);
      existingTitles.add(title.trim().toLowerCase());
      createdCount++;
      // Wait 1.5 seconds between issues to avoid secondary rate limits
      await sleep(1500);
    } catch (err) {
      console.error(`  ✗ Error creating issue: ${err.message}`);
    }
  }

  console.log('\n========================================');
  console.log(`Summary: ${createdCount} created, ${skippedCount} skipped.`);
  console.log('========================================');
}

main().catch((err) => {
  console.error('Fatal error:', err);
  process.exit(1);
});
