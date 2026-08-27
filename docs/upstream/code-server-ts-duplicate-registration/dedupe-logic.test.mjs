// Phase 7D Part 13: structural negative control for the downstream
// patch (`0001-dedupe-remote-web-extension-servers.patch`), proving its
// dedup RULE -- not merely inspecting the source and calling that a
// pass. `dedupeBuiltinsAcrossRemoteAndWeb` below is a direct,
// line-by-line transcription of the patch's own private method (same
// crate: `docs/upstream/code-server-ts-duplicate-registration/0001-...
// .patch`), reimplemented standalone so it can be exercised with plain
// `node` against synthetic fixtures, without building the full VS Code
// workbench. If the patch's real source ever changes, this must be
// re-transcribed to match -- it is deliberately NOT imported from the
// built artifact, since the built artifact is a 700MB+ bundle this test
// has no reason to depend on.
//
// Run: node docs/upstream/code-server-ts-duplicate-registration/dedupe-logic.test.mjs

const REMOTE = 'remote';
const WEB = 'web';

/** @param {{id: string, version: string, isBuiltin: boolean, server: 'remote'|'web'}[]} extensions */
function dedupeBuiltinsAcrossRemoteAndWeb(extensions) {
  const remoteBuiltins = new Map();
  for (const extension of extensions) {
    if (extension.isBuiltin && extension.server === REMOTE) {
      remoteBuiltins.set(extension.id.toLowerCase(), extension.version);
    }
  }
  return extensions.filter((extension) => {
    if (!extension.isBuiltin || extension.server !== WEB) {
      return true;
    }
    const remoteVersion = remoteBuiltins.get(extension.id.toLowerCase());
    return remoteVersion === undefined || remoteVersion !== extension.version;
  });
}

let failures = 0;
function check(name, condition) {
  if (condition) {
    console.log(`PASS: ${name}`);
  } else {
    failures += 1;
    console.log(`FAIL: ${name}`);
  }
}

// -- Required control 1 (Part 13): same identifier, DIFFERENT version,
// both builtin, remote + web -- BOTH must be preserved. This is the
// exact scenario the patch review deliberately strengthened for
// (Phase 7B-12): a version-match requirement was added specifically so
// this case is never collapsed.
{
  const input = [
    { id: 'example.extension', version: '1.0.0', isBuiltin: true, server: REMOTE },
    { id: 'example.extension', version: '2.0.0', isBuiltin: true, server: WEB },
  ];
  const result = dedupeBuiltinsAcrossRemoteAndWeb(input);
  check(
    'same id, different version, remote+web builtin -> both preserved',
    result.length === 2 &&
      result.some((e) => e.server === REMOTE && e.version === '1.0.0') &&
      result.some((e) => e.server === WEB && e.version === '2.0.0')
  );
}

// -- Required control 2 (Part 13): same identifier, SAME version, both
// builtin, remote + web -- the web duplicate must be removed, remote
// kept. This is the actual defect the patch fixes (the real-world
// `vscode.typescript-language-features` collision).
{
  const input = [
    { id: 'vscode.typescript-language-features', version: '1.0.0', isBuiltin: true, server: REMOTE },
    { id: 'vscode.typescript-language-features', version: '1.0.0', isBuiltin: true, server: WEB },
  ];
  const result = dedupeBuiltinsAcrossRemoteAndWeb(input);
  check(
    'same id, same version, remote+web builtin -> web duplicate removed',
    result.length === 1 && result[0].server === REMOTE
  );
}

// -- Negative control: web-only, non-builtin (a real user-installed
// extension that happens to only report from the web server) must never
// be touched -- the `isBuiltin` guard is load-bearing.
{
  const input = [{ id: 'someone.user-extension', version: '3.1.4', isBuiltin: false, server: WEB }];
  const result = dedupeBuiltinsAcrossRemoteAndWeb(input);
  check('web-only non-builtin (real user extension) -> untouched', result.length === 1);
}

// -- Negative control: same id + same version but on the SAME server
// twice (not remote+web) must never be collapsed -- the rule is
// specifically cross-server, not "any duplicate by id+version".
{
  const input = [
    { id: 'example.extension', version: '1.0.0', isBuiltin: true, server: REMOTE },
    { id: 'example.extension', version: '1.0.0', isBuiltin: true, server: REMOTE },
  ];
  const result = dedupeBuiltinsAcrossRemoteAndWeb(input);
  check('same id+version, both on remote (not cross-server) -> untouched', result.length === 2);
}

if (failures > 0) {
  console.error(`\n${failures} control(s) FAILED`);
  process.exit(1);
}
console.log('\nAll patch dedup logic controls PASSED');
