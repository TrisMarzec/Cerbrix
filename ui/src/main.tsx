import { render } from 'preact';

import './styles/base.css';
import { App } from './app';

const root = document.getElementById('app');
if (root === null) {
  throw new Error('index.html has no #app element');
}
render(<App />, root);
