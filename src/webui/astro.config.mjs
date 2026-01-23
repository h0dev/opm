import react from '@astrojs/react';
import relativeLinks from './links';
import tailwind from '@astrojs/tailwind';
import { defineConfig } from 'astro/config';

export default defineConfig({
	output: 'static',
	build: { 
		format: 'file', 
		assets: 'assets',
		// Enable content hashing for cache busting
		assetsPrefix: undefined,
		// Astro automatically hashes assets in production builds
	},
	integrations: [tailwind(), react(), relativeLinks()],
	vite: {
		build: {
			// Enable CSS code splitting and hashing
			cssCodeSplit: true,
			// Generate hashed filenames for better caching
			rollupOptions: {
				output: {
					// Hash all asset filenames
					assetFileNames: 'assets/[name].[hash][extname]',
					// Hash all chunk filenames
					chunkFileNames: 'assets/[name].[hash].js',
					// Hash all entry filenames
					entryFileNames: 'assets/[name].[hash].js',
				}
			}
		}
	}
});
