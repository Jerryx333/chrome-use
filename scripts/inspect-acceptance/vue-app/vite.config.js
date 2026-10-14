import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';
import { codeInspectorPlugin } from 'code-inspector-plugin';
export default defineConfig({ plugins: [codeInspectorPlugin({ bundler: 'vite', pathType: 'relative', hideConsole: true }), vue()] });
