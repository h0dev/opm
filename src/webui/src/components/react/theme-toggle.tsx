import { useStore } from '@nanostores/react';
import { $settings } from '@/store';
import { useEffect } from 'react';

export default function ThemeToggle() {
	const settings = useStore($settings);
	const currentTheme = settings.theme || 'dark';

	// Sync theme with localStorage changes (e.g., from other tabs)
	useEffect(() => {
		const syncTheme = () => {
			const settingsStr = localStorage.getItem('settings:');
			if (settingsStr) {
				try {
					const settings = JSON.parse(settingsStr);
					const theme = settings.theme || 'dark';
					document.documentElement.classList.toggle('dark', theme === 'dark');
				} catch (e) {
					console.error('Failed to parse settings:', e);
				}
			}
		};

		// Listen for storage events from other tabs
		window.addEventListener('storage', syncTheme);
		
		// Also sync on component mount to ensure correct state
		syncTheme();

		return () => {
			window.removeEventListener('storage', syncTheme);
		};
	}, []);

	// Update DOM class when theme changes
	useEffect(() => {
		document.documentElement.classList.toggle('dark', currentTheme === 'dark');
	}, [currentTheme]);

	const toggleTheme = () => {
		const newTheme = currentTheme === 'dark' ? 'light' : 'dark';
		$settings.setKey('theme', newTheme);
		
		// Update the document class immediately
		document.documentElement.classList.toggle('dark', newTheme === 'dark');
	};

	return (
		<button
			onClick={toggleTheme}
			className="flex items-center justify-center p-2 rounded-lg transition-colors duration-200 hover:bg-gray-100 dark:bg-zinc-800/50 dark:hover:bg-gray-100 dark:bg-zinc-800/50 hover:bg-gray-200 focus:outline-none focus:ring-2 focus:ring-sky-500"
			aria-label={currentTheme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
			title={currentTheme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
		>
			{currentTheme === 'dark' ? (
				// Sun icon for light mode
				<svg className="w-5 h-5 text-gray-500 dark:text-zinc-400 hover:text-yellow-400 transition-colors" fill="none" viewBox="0 0 24 24" stroke="currentColor">
					<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 3v1m0 16v1m9-9h-1M4 12H3m15.364 6.364l-.707-.707M6.343 6.343l-.707-.707m12.728 0l-.707.707M6.343 17.657l-.707.707M16 12a4 4 0 11-8 0 4 4 0 018 0z" />
				</svg>
			) : (
				// Moon icon for dark mode
				<svg className="w-5 h-5 text-gray-600 hover:text-blue-600 transition-colors" fill="none" viewBox="0 0 24 24" stroke="currentColor">
					<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M20.354 15.354A9 9 0 018.646 3.646 9.003 9.003 0 0012 21a9.003 9.003 0 008.354-5.646z" />
				</svg>
			)}
		</button>
	);
}
