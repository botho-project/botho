// Babel configuration for the Botho mobile app (Expo Router).
//
// `babel-preset-expo` provides the React-Native + TypeScript + Expo Router
// transforms. This config is required both for the Metro bundler and for the
// `jest-expo` test preset to transform `.ts` / `.tsx` sources.
module.exports = function babelConfig(api) {
  api.cache(true);
  return {
    presets: ["babel-preset-expo"],
  };
};
