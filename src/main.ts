import { mount } from 'svelte';
import './styles/base.css';
import App from './App.svelte';
import { startTheme } from './lib/state/theme.svelte';

const stopTheme = startTheme();
const app = mount(App, { target: document.getElementById('app')! });

if (import.meta.hot) import.meta.hot.dispose(stopTheme);

export default app;
