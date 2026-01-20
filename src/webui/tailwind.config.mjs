/** @type {import('tailwindcss').Config} */
export default {
	content: ['./src/**/*.{astro,html,js,jsx,md,mdx,svelte,ts,tsx,vue}'],
	theme: {
		extend: {
			fontFamily: {
				sans: ['Inter var', ...require('tailwindcss/defaultTheme').fontFamily.sans]
			},
			animation: {
				progress: 'progress 1s infinite linear'
			},
			keyframes: {
				progress: {
					'0%': { transform: ' translateX(0) scaleX(0)' },
					'40%': { transform: 'translateX(0) scaleX(0.4)' },
					'100%': { transform: 'translateX(100%) scaleX(0.5)' }
				}
			},
			transformOrigin: {
				'left-right': '0% 50%'
			},
			boxShadow: {
				'glow': '0 0 20px rgba(168, 85, 247, 0.4), 0 0 40px rgba(168, 85, 247, 0.2)'
			}
		}
	},
	plugins: [require('@tailwindcss/forms')]
};
