// Screen share has been paused since 0.4.58, and react-native-reanimated / react-native-worklets
// are unused in app code (kept in package.json only to satisfy expo-router's peer dependency tree
// and package-lock.json sync). Keep their native modules excluded from autolinking on both iOS and
// Android so release builds do not bundle unused native binaries while keeping screen-share JS code
// and tests intact for future re-enablement.
module.exports = {
  dependencies: {
    'react-native-webrtc': { platforms: { ios: null, android: null } },
    'react-native-zstd': { platforms: { ios: null, android: null } },
    'react-native-nitro-modules': { platforms: { ios: null, android: null } },
    'react-native-reanimated': { platforms: { ios: null, android: null } },
    'react-native-worklets': { platforms: { ios: null, android: null } },
  },
};
