import { api } from '@/api';
import { useEffect, Fragment, useState } from 'react';
import Loader from '@/components/react/loader';
import Header from '@/components/react/header';
import ToastContainer from '@/components/react/toast';
import { useToast } from '@/components/react/useToast';

// Toggle switch common styles - rely on CSS from styles.css for positioning and transforms
// Mobile: h-5 w-9 (1.25rem/20px × 2.25rem/36px) - height matches pin diameter for perfect rounded edges
// Desktop: h-8 w-13 (2rem/32px × 3.25rem/52px) creates iOS-style pill shape with 1.625:1 ratio
const TOGGLE_BASE_CLASSES = "relative inline-flex h-5 w-9 sm:h-8 sm:w-13 flex-shrink-0 items-center rounded-full transition-colors duration-200 focus:outline-none focus:ring-2 focus:ring-blue-500 focus:ring-offset-2 focus:ring-offset-zinc-900";
const TOGGLE_PIN_CLASSES = "inline-block h-5 w-5 sm:h-7 sm:w-7 rounded-full bg-white transition-transform duration-200";
const getToggleClasses = (enabled: boolean) => `${TOGGLE_BASE_CLASSES} ${enabled ? 'bg-blue-600' : 'bg-zinc-700'}`;
const getTogglePinClasses = (enabled: boolean) => TOGGLE_PIN_CLASSES;

const NotificationSettings = (props: { base: string }) => {
	const { toasts, closeToast, success, error } = useToast();
	const [loading, setLoading] = useState(true);
	const [saving, setSaving] = useState(false);
	const [testing, setTesting] = useState(false);
	const [settings, setSettings] = useState({
		enabled: false,
		events: {
			agent_connect: false,
			agent_disconnect: false,
			process_start: false,
			process_stop: false,
			process_crash: false,
			process_restart: false
		},
		channels: [] as string[]
	});
	const [newChannel, setNewChannel] = useState('');

	async function fetchSettings() {
		try {
			const response = await api.get(`${props.base}/daemon/config/notifications`).json();
			setSettings(response || settings);
		} catch (error) {
			console.error('Failed to fetch notification settings:', error);
		} finally {
			setLoading(false);
		}
	}

	async function saveSettings() {
		setSaving(true);
		try {
			await api.post(`${props.base}/daemon/config/notifications`, {
				json: settings
			});
			success('Notification settings saved successfully');
		} catch (err) {
			error('Failed to save settings: ' + (err as Error).message);
		} finally {
			setSaving(false);
		}
	}

	async function testNotification() {
		setTesting(true);
		try {
			await api.post(`${props.base}/daemon/test-notification`, {
				json: {
					title: 'OPM Test Notification',
					message: 'This is a test notification from OPM'
				}
			});
			success('Test notification sent! Check your notification channels.');
		} catch (err) {
			error('Failed to send test notification: ' + (err as Error).message);
		} finally {
			setTesting(false);
		}
	}

	const addChannel = () => {
		if (newChannel.trim()) {
			setSettings({
				...settings,
				channels: [...settings.channels, newChannel.trim()]
			});
			setNewChannel('');
		}
	};

	const removeChannel = (index: number) => {
		setSettings({
			...settings,
			channels: settings.channels.filter((_, i) => i !== index)
		});
	};

	useEffect(() => {
		fetchSettings();
	}, []);

	if (loading) {
		return <Loader />;
	}

	return (
		<Fragment>
			<ToastContainer toasts={toasts} onClose={closeToast} />
			<Header name="Notification Settings" description="Configure desktop notifications and external notification channels.">
				<div className="flex gap-2">
					<button
						type="button"
						onClick={testNotification}
						disabled={testing || !settings.enabled || settings.channels.length === 0}
						className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-emerald-700 hover:border-emerald-600 bg-emerald-600 text-white hover:bg-emerald-700 disabled:bg-zinc-700 disabled:border-zinc-700 disabled:cursor-not-allowed px-4 py-2 text-sm font-semibold rounded-lg">
						{testing ? 'Testing...' : 'Test Notification'}
					</button>
					<button
						type="button"
						onClick={saveSettings}
						disabled={saving}
						className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-blue-700 hover:border-blue-600 bg-blue-600 text-white hover:bg-blue-700 disabled:bg-zinc-700 disabled:border-zinc-700 disabled:cursor-not-allowed px-4 py-2 text-sm font-semibold rounded-lg">
						{saving ? 'Saving...' : 'Save Settings'}
					</button>
					<button
						type="button"
						onClick={fetchSettings}
						className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-900 hover:border-zinc-800 bg-zinc-950 text-zinc-50 hover:bg-zinc-900 px-4 py-2 text-sm font-semibold rounded-lg">
						Refresh
					</button>
				</div>
			</Header>

			<div className="space-y-6">
				{/* Master Toggle */}
				<div className="bg-zinc-900 border border-zinc-800 rounded-lg p-6">
					<div className="flex items-center justify-between gap-4">
						<div className="flex-1">
							<h3 className="text-lg font-semibold text-zinc-200">Enable Notifications</h3>
							<p className="text-sm text-zinc-400 mt-1">
								Master switch for all notifications
							</p>
						</div>
						<div className="flex-shrink-0">
							<button
								type="button"
								onClick={() => setSettings({ ...settings, enabled: !settings.enabled })}
								className={getToggleClasses(settings.enabled)}
								role="switch"
								aria-checked={settings.enabled}>
								<span className={getTogglePinClasses(settings.enabled)} />
							</button>
						</div>
					</div>
				</div>

				{/* Event Types */}
				<div className="bg-zinc-900 border border-zinc-800 rounded-lg p-6">
					<h3 className="text-lg font-semibold text-zinc-200 mb-4">Event Types</h3>
					<div className="space-y-4">
						{Object.entries(settings.events).map(([key, value]) => (
							<div key={key} className="flex items-center justify-between gap-4">
								<div className="flex-1">
									<div className="text-sm font-medium text-zinc-200">
										{key.split('_').map(w => w.charAt(0).toUpperCase() + w.slice(1)).join(' ')}
									</div>
									<div className="text-xs text-zinc-400">
										Notify when {key.replace('_', ' ')} occurs
									</div>
								</div>
								<div className="flex-shrink-0">
									<button
										type="button"
										onClick={() => setSettings({
											...settings,
											events: {
												...settings.events,
												[key]: !value
											}
										})}
										disabled={!settings.enabled}
										className={`${getToggleClasses(value && settings.enabled)} disabled:opacity-50 disabled:cursor-not-allowed`}
										role="switch"
										aria-checked={value}>
										<span className={getTogglePinClasses(value)} />
									</button>
								</div>
							</div>
						))}
					</div>
				</div>

				{/* Notification Channels */}
				<div className="bg-zinc-900 border border-zinc-800 rounded-lg p-6">
					<h3 className="text-lg font-semibold text-zinc-200 mb-2">Notification Channels</h3>
					<p className="text-sm text-zinc-400 mb-4">
						Add external notification channels using Shoutrrr URLs (e.g., Discord, Slack, Telegram)
					</p>
					
					{/* Input field for adding new notification channels */}
					<div className="flex gap-2 mb-4">
						<input
							id="notification-channel-input"
							type="text"
							value={newChannel}
							onChange={(e) => setNewChannel(e.target.value)}
							onKeyPress={(e) => e.key === 'Enter' && addChannel()}
							placeholder="discord://token@id or slack://token:token@channel"
							className="flex-1 px-3 py-1.5 text-sm bg-zinc-800 border border-zinc-700 rounded-lg text-zinc-200 placeholder-zinc-500 focus:outline-none focus:ring-2 focus:ring-blue-500"
						/>
						<button
							onClick={addChannel}
							disabled={!newChannel.trim() || !settings.enabled}
							className="px-3 py-1.5 text-sm bg-blue-600 hover:bg-blue-700 disabled:bg-zinc-700 disabled:cursor-not-allowed text-white rounded-lg font-medium transition">
							Add
						</button>
					</div>

					{/* Channel List */}
					{settings.channels.length === 0 ? (
						<div className="text-center py-4 text-zinc-400 text-sm">
							No channels configured
						</div>
					) : (
						<div className="space-y-2">
							{settings.channels.map((channel, index) => (
								<div key={index} className="flex items-center justify-between bg-zinc-800 border border-zinc-700 rounded-lg p-3">
									<div className="flex-1 font-mono text-sm text-zinc-300 truncate mr-4">
										{channel}
									</div>
									<button
										onClick={() => removeChannel(index)}
										className="text-red-400 hover:text-red-300 text-sm font-medium transition">
										Remove
									</button>
								</div>
							))}
						</div>
					)}

					<div className="mt-4 p-3 bg-zinc-800 border border-zinc-700 rounded-lg">
						<div className="text-xs text-zinc-400">
							<strong className="text-zinc-300">Examples:</strong>
							<ul className="list-disc list-inside mt-2 space-y-1">
								<li><code className="text-zinc-300">discord://token@id</code> - Discord webhook</li>
								<li><code className="text-zinc-300">slack://token:token@channel</code> - Slack webhook</li>
								<li><code className="text-zinc-300">telegram://token@telegram?chats=@chat</code> - Telegram</li>
							</ul>
							<p className="mt-2">
								See <a href="https://containrrr.dev/shoutrrr/" target="_blank" rel="noopener noreferrer" className="text-blue-400 hover:underline">Shoutrrr documentation</a> for more formats.
							</p>
						</div>
					</div>
				</div>
			</div>
		</Fragment>
	);
};

export default NotificationSettings;
