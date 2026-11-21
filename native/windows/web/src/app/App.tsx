import { useEffect } from 'react';
import { Shell } from '../shell/Shell';
import { applyText } from '../theme/text';
import { saveTheme, themeName } from '../theme/tokens';
import { shallowEqual, useStoreSelector } from './context';

export function App() {
  const { snapshotTheme, text } = useStoreSelector(
    (state) => ({ snapshotTheme: state.snapshot ? themeName(state.snapshot.theme) : null, text: state.text }),
    shallowEqual,
  );
  useEffect(() => { applyText(document, text); }, [text]);
  useEffect(() => {
    if (!snapshotTheme) return;
    document.documentElement.dataset.theme = snapshotTheme;
    saveTheme(snapshotTheme);
  }, [snapshotTheme]);
  return <Shell />;
}
