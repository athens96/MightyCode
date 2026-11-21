import { useEffect } from 'react';
import { Shell } from '../shell/Shell';
import { applyText } from '../theme/text';
import { themeName } from '../theme/tokens';
import { shallowEqual, useStoreSelector } from './context';

export function App() {
  const { theme, text } = useStoreSelector(
    (state) => ({ theme: themeName(state.snapshot?.theme), text: state.text }),
    shallowEqual,
  );
  useEffect(() => { applyText(document, text); }, [text]);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);
  return <Shell />;
}
