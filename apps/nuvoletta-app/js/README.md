# Editor bundle

`../assets/editor.js` is a vendored, minified build of `entry.js` (CodeMirror 6 plus the Markdown
language) so that the app never loads script from a network. Rebuild after changing `entry.js` or
the dependencies:

```sh
cd apps/nuvoletta-app/js
npm ci          # installs exactly what package-lock.json pins
npm run build   # esbuild -> ../assets/editor.js
```

Review `package-lock.json` changes like code (PLAN.md section 5.4): the lock file pins every
package with an integrity hash. `node_modules/` is git-ignored.
