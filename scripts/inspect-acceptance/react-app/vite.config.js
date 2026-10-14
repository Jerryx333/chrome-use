import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { codeInspectorPlugin } from 'code-inspector-plugin';
const isTestBuild = process.env.DEPLOY_ENV === 'test';
export default defineConfig({
  plugins: [codeInspectorPlugin({ bundler: 'vite', pathType: 'relative', dev: () => process.env.NODE_ENV !== 'production' || isTestBuild, hideConsole: true }), react()],
});
