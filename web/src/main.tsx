import { render } from 'preact';
import { App } from './App';
import { applyTheme, initialTheme } from './ui/router';
import { lang } from './i18n';
import './styles.css';

applyTheme(initialTheme());
document.documentElement.lang = lang;
render(<App />, document.getElementById('app')!);
