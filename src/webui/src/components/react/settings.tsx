import { api } from '@/api';
import { useEffect, useState } from 'react';
import Loader from '@/components/react/loader';
import Header from '@/components/react/header';
import ToastContainer from '@/components/react/toast';
import { useToast } from '@/components/react/useToast';

interface SecurityConfig {
	enabled: boolean;
	token: string;
}

interface NotificationConfig {
	enabled: boolean;
	events: {
		agent_connect: boolean;
		agent_disconnect: boolean;
		process_start: boolean;
		process_stop: boolean;
		process_crash: boolean;
		process_restart: boolean;
		process_delete: boolean;
	};
	channels: string[];
}

const SettingsPage = (props: { base: string }) => {
	const { toasts, closeToast, success, error } = useToast();
	const [loading, setLoading] = useState(true);
	const [saving, setSaving] = useState(false);
	
	// Security Settings
	const [securityConfig, setSecurityConfig] = useState<SecurityConfig>({
		enabled: false,
		token: '',
	});
	
	// Notification Settings
	const [notificationConfig, setNotificationConfig] = useState<NotificationConfig>({
		enabled: false,
		events: {
			agent_connect: false,
			agent_disconnect: false,
			process_start: false,
			process_stop: false,
			process_crash: false,
			process_restart: false,
			process_delete: false,
		},
		channels: [],
	});
	
	const [newChannel, setNewChannel] = useState('');
	const [showTokenPassword, setShowTokenPassword] = useState(false);

	useEffect(() => {
		fetchSettings();
	}, []);

	const fetchSettings = async () => {
		try {
			// Fetch security config
			const securityRes = await api.get(`daemon/config/security`).json<SecurityConfig>();
			setSecurityConfig(securityRes);
			
			// Fetch notification config
			const notificationRes = await api.get(`daemon/config/notifications`).json<NotificationConfig>();
			setNotificationConfig(notificationRes);
			
			setLoading(false);
		} catch (err: any) {
			error('Failed to load settings');
			console.error('Error fetching settings:', err);
			setLoading(false);
		}
	};

	const saveSecuritySettings = async () => {
		setSaving(true);
		try {
			await api.post(`daemon/config/security`, {
				json: securityConfig,
			}).json();
			success('Security settings saved! Restart daemon for token changes to take effect.');
		} catch (err: any) {
			error('Failed to save security settings');
			console.error('Error saving security settings:', err);
		} finally {
			setSaving(false);
		}
	};

	const saveNotificationSettings = async () => {
		setSaving(true);
		try {
			await api.post(`daemon/config/notifications`, {
				json: notificationConfig,
			}).json();
			success('Notification settings saved successfully');
		} catch (err: any) {
			error('Failed to save notification settings');
			console.error('Error saving notification settings:', err);
		} finally {
			setSaving(false);
		}
	};

	const testNotification = async () => {
		setSaving(true);
		try {
			await api.post(`daemon/test-notification`, {
				json: {
					title: 'Test Notification',
					message: 'This is a test notification from OPM settings',
				},
			}).json();
			success('Test notification sent successfully');
		} catch (err: any) {
			error('Failed to send test notification');
			console.error('Error sending test notification:', err);
		} finally {
			setSaving(false);
		}
	};

	const addChannel = () => {
		if (newChannel.trim() && !notificationConfig.channels.includes(newChannel.trim())) {
			setNotificationConfig({
				...notificationConfig,
				channels: [...notificationConfig.channels, newChannel.trim()],
			});
			setNewChannel('');
		}
	};

	const removeChannel = (channel: string) => {
		setNotificationConfig({
			...notificationConfig,
			channels: notificationConfig.channels.filter(c => c !== channel),
		});
	};

	const generateToken = () => {
		// Generate a random secure token
		const chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
		let token = '';
		for (let i = 0; i < 32; i++) {
			token += chars.charAt(Math.floor(Math.random() * chars.length));
		}
		setSecurityConfig({ ...securityConfig, token });
	};

	if (loading) {
		return <Loader />;
	}

	return (
		<>
			<ToastContainer toasts={toasts} onClose={closeToast} />
			<div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
				<Header name="Settings" description="Manage authentication tokens and notification channels">
					<></>
				</Header>

				{/* Security Settings */}
				<div className="mt-8 bg-white dark:bg-zinc-900/20 border border-gray-200 dark:border-zinc-800/50 rounded-lg shadow-sm">
					<div className="p-6">
						<h2 className="text-lg font-semibold text-gray-900 dark:text-white mb-4">
							Security & Authentication
						</h2>
						
						<div className="space-y-4">
							<div className="flex items-center">
								<input
									type="checkbox"
									id="security-enabled"
									checked={securityConfig.enabled}
									onChange={(e) => setSecurityConfig({ ...securityConfig, enabled: e.target.checked })}
									className="h-4 w-4 text-sky-600 focus:ring-sky-500 border-gray-300 rounded"
								/>
								<label htmlFor="security-enabled" className="ml-2 text-sm text-gray-700 dark:text-zinc-300">
									Enable API authentication
								</label>
							</div>
							
							<div>
								<label className="block text-sm font-medium text-gray-700 dark:text-zinc-300 mb-2">
									API Token
								</label>
								<div className="flex gap-2">
									<div className="relative flex-1">
										<input
											type={showTokenPassword ? 'text' : 'password'}
											value={securityConfig.token}
											onChange={(e) => setSecurityConfig({ ...securityConfig, token: e.target.value })}
											className="w-full px-3 py-2 border border-gray-300 dark:border-zinc-700 rounded-lg bg-white dark:bg-zinc-800 text-gray-900 dark:text-white focus:ring-2 focus:ring-sky-500 focus:border-transparent"
											placeholder="Enter API token"
										/>
										<button
											type="button"
											onClick={() => setShowTokenPassword(!showTokenPassword)}
											className="absolute right-2 top-1/2 -translate-y-1/2 text-gray-500 hover:text-gray-700 dark:text-zinc-400 dark:hover:text-zinc-200"
										>
											{showTokenPassword ? (
												<svg className="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
													<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.88 9.88l-3.29-3.29m7.532 7.532l3.29 3.29M3 3l3.59 3.59m0 0A9.953 9.953 0 0112 5c4.478 0 8.268 2.943 9.543 7a10.025 10.025 0 01-4.132 5.411m0 0L21 21" />
												</svg>
											) : (
												<svg className="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
													<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
													<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
												</svg>
											)}
										</button>
									</div>
									<button
										onClick={generateToken}
										className="px-4 py-2 bg-gray-600 hover:bg-gray-700 text-white rounded-lg text-sm font-medium transition-colors"
									>
										Generate
									</button>
								</div>
								<p className="mt-2 text-xs text-gray-500 dark:text-zinc-400">
									Use this token as Bearer authentication for API requests
								</p>
							</div>
							
							<div className="pt-4">
								<button
									onClick={saveSecuritySettings}
									disabled={saving}
									className="px-4 py-2 bg-sky-600 hover:bg-sky-700 disabled:bg-gray-400 text-white rounded-lg font-medium transition-colors"
								>
									{saving ? 'Saving...' : 'Save Security Settings'}
								</button>
							</div>
						</div>
					</div>
				</div>

				{/* Notification Settings */}
				<div className="mt-8 bg-white dark:bg-zinc-900/20 border border-gray-200 dark:border-zinc-800/50 rounded-lg shadow-sm">
					<div className="p-6">
						<h2 className="text-lg font-semibold text-gray-900 dark:text-white mb-4">
							Notifications
						</h2>
						
						<div className="space-y-4">
							<div className="flex items-center">
								<input
									type="checkbox"
									id="notifications-enabled"
									checked={notificationConfig.enabled}
									onChange={(e) => setNotificationConfig({ ...notificationConfig, enabled: e.target.checked })}
									className="h-4 w-4 text-sky-600 focus:ring-sky-500 border-gray-300 rounded"
								/>
								<label htmlFor="notifications-enabled" className="ml-2 text-sm text-gray-700 dark:text-zinc-300">
									Enable notifications
								</label>
							</div>
							
							<div>
								<h3 className="text-sm font-medium text-gray-700 dark:text-zinc-300 mb-2">Event Types</h3>
								<div className="space-y-2">
									{Object.entries(notificationConfig.events).map(([event, enabled]) => (
										<div key={event} className="flex items-center">
											<input
												type="checkbox"
												id={`event-${event}`}
												checked={enabled}
												onChange={(e) => setNotificationConfig({
													...notificationConfig,
													events: { ...notificationConfig.events, [event]: e.target.checked }
												})}
												className="h-4 w-4 text-sky-600 focus:ring-sky-500 border-gray-300 rounded"
											/>
											<label htmlFor={`event-${event}`} className="ml-2 text-sm text-gray-700 dark:text-zinc-300 capitalize">
												{event.replace(/_/g, ' ')}
											</label>
										</div>
									))}
								</div>
							</div>
							
							<div>
								<h3 className="text-sm font-medium text-gray-700 dark:text-zinc-300 mb-2">
									Notification Channels
								</h3>
								<p className="text-xs text-gray-500 dark:text-zinc-400 mb-2">
									Supported formats: discord://token@id, telegram://token@telegram?chats=@chat_id, slack://webhook_url
								</p>
								<div className="flex gap-2 mb-2">
									<input
										type="text"
										value={newChannel}
										onChange={(e) => setNewChannel(e.target.value)}
										onKeyPress={(e) => e.key === 'Enter' && addChannel()}
										placeholder="e.g., telegram://BOT_TOKEN@telegram?chats=@CHAT_ID"
										className="flex-1 px-3 py-2 border border-gray-300 dark:border-zinc-700 rounded-lg bg-white dark:bg-zinc-800 text-gray-900 dark:text-white text-sm focus:ring-2 focus:ring-sky-500 focus:border-transparent"
									/>
									<button
										onClick={addChannel}
										className="px-4 py-2 bg-sky-600 hover:bg-sky-700 text-white rounded-lg text-sm font-medium transition-colors"
									>
										Add
									</button>
								</div>
								
								<div className="space-y-2 mt-3">
									{notificationConfig.channels.map((channel, index) => (
										<div key={index} className="flex items-center justify-between p-2 bg-gray-50 dark:bg-zinc-800/50 rounded-lg">
											<code className="text-xs text-gray-700 dark:text-zinc-300 flex-1 overflow-x-auto">
												{channel}
											</code>
											<button
												onClick={() => removeChannel(channel)}
												className="ml-2 text-red-600 hover:text-red-700 dark:text-red-400 dark:hover:text-red-300"
											>
												<svg className="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
													<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
												</svg>
											</button>
										</div>
									))}
								</div>
							</div>
							
							<div className="pt-4 flex gap-2">
								<button
									onClick={saveNotificationSettings}
									disabled={saving}
									className="px-4 py-2 bg-sky-600 hover:bg-sky-700 disabled:bg-gray-400 text-white rounded-lg font-medium transition-colors"
								>
									{saving ? 'Saving...' : 'Save Notification Settings'}
								</button>
								<button
									onClick={testNotification}
									disabled={saving || !notificationConfig.enabled}
									className="px-4 py-2 bg-gray-600 hover:bg-gray-700 disabled:bg-gray-400 text-white rounded-lg font-medium transition-colors"
								>
									Send Test Notification
								</button>
							</div>
						</div>
					</div>
				</div>
			</div>
		</>
	);
};

export default SettingsPage;
