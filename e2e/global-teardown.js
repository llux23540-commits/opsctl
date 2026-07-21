const fs = require('fs');
const path = require('path');

// Remove throwaway e2e DBs (and sqlite sidecar files). Sweeps ALL target/e2e-*.db
// so interrupted runs don't accumulate — every e2e-<port>.db is disposable.
module.exports = async () => {
  const targetDir = path.resolve(__dirname, '..', 'target');
  try {
    for (const f of fs.readdirSync(targetDir)) {
      if (/^e2e-\d+\.db(-wal|-shm)?$/.test(f)) {
        try { fs.rmSync(path.join(targetDir, f), { force: true }); } catch (_) { /* ignore */ }
      }
    }
  } catch (_) { /* target dir missing → nothing to clean */ }
};
