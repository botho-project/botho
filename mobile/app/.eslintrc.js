// ESLint configuration for the Botho mobile app.
//
// Uses the Expo shared config (eslint-config-expo) which bundles the
// React / React-Native / TypeScript rules appropriate for an Expo Router app.
// ESLint 8 resolves this `.eslintrc.js` (eslintrc format).
module.exports = {
  root: true,
  extends: ["expo"],
  ignorePatterns: [
    "node_modules/",
    "ios/",
    "android/",
    ".expo/",
    "dist/",
  ],
};
