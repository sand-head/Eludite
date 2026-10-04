// The ESLint flat configuration of Eludite's web corpus (brief 0050): JavaScript files only (ESLint's own parser
// does not read TypeScript), with one fixable rule so src/lint.js has a violation and a fix.
export default [
  {
    files: ["**/*.js"],
    languageOptions: { ecmaVersion: "latest", sourceType: "module" },
    rules: { "prefer-const": "error" },
  },
];
