// ESLint configuration for the Botho mobile app.
//
// Uses the Expo shared config (eslint-config-expo) which bundles the
// React / React-Native / TypeScript rules appropriate for an Expo Router app.
// ESLint 9 resolves this `eslint.config.js` (flat config format).
const expoConfig = require("eslint-config-expo/flat");
const { defineConfig } = require("eslint/config");

module.exports = defineConfig([
  ...expoConfig,
  {
    ignores: [
      "node_modules/",
      "ios/",
      "android/",
      ".expo/",
      "dist/",
    ],
  },
]);
