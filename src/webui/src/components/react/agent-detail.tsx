import { SSE, api, headers } from '@/api';
import { useEffect, Fragment, useState } from 'react';
import Loader from '@/components/react/loader';
import Header from '@/components/react/header';
import { useArray, classNames, startDuration, formatMemory, isLocalAgent } from '@/helpers';

// SSE connection timeout in milliseconds
const SSE_TIMEOUT_MS = 10000; // 10 seconds

const AgentDetail = (props: { agentId: string; base: string }) => {
	const [agent, setAgent] = useState<any>(null);
	const [processes, setProcesses] = useState<any[]>([]);
	const [loading, setLoading] = useState(true);
	const [error, setError] = useState<string | null>(null);
	const [retryTrigger, setRetryTrigger] = useState(0);

	// Log component mount for debugging
	useEffect(() => {
		if (import.meta.env.DEV) {
			console.log('[AgentDetail] Component mounted with agentId:', props.agentId);
		}
		return () => {
			if (import.meta.env.DEV) {
				console.log('[AgentDetail] Component unmounting');
			}
		};
	}, []);

	// Fix instruction for API endpoint issues
	const API_ENDPOINT_FIX = (
		<>
			<strong>To fix:</strong> Restart the agent with:{' '}
			<code className="bg-gray-100 dark:bg-zinc-800 px-1 py-0.5 rounded">
				opm agent connect {'<'}server-url{'>'} --api-address {'<'}agent-ip-or-hostname{'>'}
			</code>
		</>
	);

	const badge = {
		online: 'bg-emerald-400/10 text-emerald-400',
		offline: 'bg-red-500/10 text-red-500',
		running: 'bg-emerald-700/40 text-emerald-400',
		stopped: 'bg-red-700/40 text-red-400'
	};

	const handleRetry = () => {
		setLoading(true);
		setError(null);
		setRetryTrigger(prev => prev + 1);
	};

	useEffect(() => {
		// Use Server-Sent Events for real-time updates with authentication
		setLoading(true);
		setError(null);
		
		// Add timeout to prevent infinite loading
		const timeoutId = setTimeout(() => {
			if (import.meta.env.DEV) {
				console.log(`SSE connection timeout after ${SSE_TIMEOUT_MS / 1000} seconds`);
			}
			setError(`Connection timeout - agent did not respond within ${SSE_TIMEOUT_MS / 1000} seconds`);
			setLoading(false);
		}, SSE_TIMEOUT_MS);
		
		const source = new SSE(`${props.base}/live/agent/${props.agentId}`, { headers });
		
		source.onmessage = (event) => {
			// Clear timeout once we receive data
			clearTimeout(timeoutId);
			
			try {
				const data = JSON.parse(event.data);
				
				if (import.meta.env.DEV) {
					console.log('SSE data received:', data);
				}
				
				if (data.error) {
					setError(data.error);
					setLoading(false);
					source.close();
				} else {
					setAgent(data.agent);
					setProcesses(Array.isArray(data.processes) ? data.processes : []);
					setLoading(false);
				}
			} catch (err) {
				console.error('Failed to parse SSE data:', err);
				setError('Failed to parse server data');
				setLoading(false);
			}
		};
		
		source.onerror = (err) => {
			clearTimeout(timeoutId);
			console.error('SSE connection error:', err);
			setError('Lost connection to server. Click Retry to reconnect.');
			setLoading(false);
		};
		
		source.stream();
		
		return () => {
			clearTimeout(timeoutId);
			source.close();
		};
	}, [props.agentId, retryTrigger]);

	if (loading) {
		if (import.meta.env.DEV) {
			console.log('[AgentDetail] Rendering loading state');
		}
		return (
			<div className="min-h-screen flex items-center justify-center">
				<Loader />
			</div>
		);
	}

	if (error || !agent) {
		if (import.meta.env.DEV) {
			console.log('[AgentDetail] Rendering error state:', { error, hasAgent: !!agent });
		}
		return (
			<Fragment>
				<Header name="Agent Details" description="Detailed information about this agent and its processes.">
					<div className="flex gap-2">
						<button
							type="button"
							onClick={handleRetry}
							className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-900 hover:border-gray-200 dark:border-zinc-800 bg-zinc-950 text-gray-900 dark:text-zinc-50 hover:bg-white dark:bg-zinc-900 px-4 py-2 text-sm font-semibold rounded-lg">
							Retry
						</button>
						<a
							href={`${props.base}/servers`}
							className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-gray-300 dark:border-zinc-700 hover:border-zinc-600 bg-gray-100 dark:bg-zinc-800 text-gray-900 dark:text-zinc-50 hover:bg-gray-200 dark:bg-zinc-700 px-4 py-2 text-sm font-semibold rounded-lg">
							Back to Agents
						</a>
					</div>
				</Header>
				<div className="bg-white dark:bg-zinc-900 border border-gray-200 dark:border-zinc-800 rounded-lg p-6">
					<div className="text-center py-12 px-4">
						<div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-red-500/10 mb-4">
							<svg className="w-8 h-8 text-red-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
								<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
							</svg>
						</div>
						<div className="text-red-400 text-xl font-semibold mb-3">Failed to Load Agent Details</div>
						<div className="text-gray-600 dark:text-zinc-300 text-base mb-6 max-w-md mx-auto">
							{error || 'Agent not found or connection failed'}
						</div>
						<div className="text-gray-900 dark:text-gray-400 dark:text-zinc-500 text-sm">
							<p className="mb-2">Agent ID: <code className="bg-gray-100 dark:bg-zinc-800 px-2 py-1 rounded text-gray-600 dark:text-zinc-300">{props.agentId}</code></p>
							<p>Make sure the agent is connected and running.</p>
						</div>
					</div>
				</div>
			</Fragment>
		);
	}

	// Backend sends last_seen as seconds since UNIX epoch
	// Heartbeat interval is 30s by default, so we use 60s threshold (2x) to account for network delays
	const isOnline = agent.last_seen && 
		(Date.now() - agent.last_seen * 1000) < 60000; // 60 seconds threshold (2x 30s heartbeat interval)

	// Check if resource usage card should be shown (at least one metric available)
	const resourceUsage = agent.system_info?.resource_usage;
	const hasResourceMetrics = resourceUsage && [
		resourceUsage.cpu_usage,
		resourceUsage.memory_percent,
		resourceUsage.disk_percent,
		resourceUsage.load_avg_1,
		resourceUsage.load_avg_5,
		resourceUsage.load_avg_15
	].some(metric => metric != null);

	if (import.meta.env.DEV) {
		console.log('[AgentDetail] Rendering agent details:', { 
			agentName: agent.name, 
			agentId: agent.id, 
			hasResourceMetrics,
			processCount: processes.length 
		});
	}

	return (
		<Fragment>
			<Header name={`Agent: ${agent.name}`} description="Detailed information about this agent and its processes.">
				<div className="flex gap-2">
					<button
						type="button"
						onClick={handleRetry}
						className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-900 hover:border-gray-200 dark:border-zinc-800 bg-zinc-950 text-gray-900 dark:text-zinc-50 hover:bg-white dark:bg-zinc-900 px-4 py-2 text-sm font-semibold rounded-lg">
						Refresh
					</button>
					<a
						href={`${props.base}/servers`}
						className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-gray-300 dark:border-zinc-700 hover:border-zinc-600 bg-gray-100 dark:bg-zinc-800 text-gray-900 dark:text-zinc-50 hover:bg-gray-200 dark:bg-zinc-700 px-4 py-2 text-sm font-semibold rounded-lg">
						Back to Agents
					</a>
				</div>
			</Header>

			{/* Agent Information Card */}
			<div className="bg-white dark:bg-zinc-900 border border-gray-200 dark:border-zinc-800 rounded-lg p-6 mb-6">
				<h2 className="text-lg font-semibold text-gray-700 dark:text-zinc-200 mb-4">Agent Information</h2>
				
				{/* Warning if agent doesn't have API endpoint */}
				{!agent.api_endpoint && !isLocalAgent(agent) && (
					<div className="mb-4 p-3 bg-amber-900/20 border border-amber-700/50 rounded-lg">
						<div className="flex items-start gap-3">
							<svg className="w-5 h-5 text-amber-400 mt-0.5 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
								<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
							</svg>
							<div>
								<div className="text-amber-400 font-medium text-sm">Limited Functionality - No API Endpoint</div>
								<div className="text-gray-600 dark:text-zinc-300 text-sm mt-1">
									This agent doesn't have an accessible API endpoint configured. Process management actions (start, stop, restart) will not be available.
								</div>
								<div className="text-gray-500 dark:text-zinc-400 text-xs mt-2">
									{API_ENDPOINT_FIX}
								</div>
							</div>
						</div>
					</div>
				)}

				{/* Warning if agent has localhost API endpoint (unreachable) */}
				{agent.api_endpoint && (
					agent.api_endpoint.includes('://localhost:') || 
					agent.api_endpoint.includes('://localhost/') ||
					agent.api_endpoint.endsWith('://localhost')
				) && !isLocalAgent(agent) && (
					<div className="mb-4 p-3 bg-red-900/20 border border-red-700/50 rounded-lg">
						<div className="flex items-start gap-3">
							<svg className="w-5 h-5 text-red-400 mt-0.5 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
								<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
							</svg>
							<div>
								<div className="text-red-400 font-medium text-sm">Unreachable API Endpoint</div>
								<div className="text-gray-600 dark:text-zinc-300 text-sm mt-1">
									This agent's API endpoint uses "localhost" which is not accessible from the server. Process management actions will fail.
								</div>
								<div className="text-gray-500 dark:text-zinc-400 text-xs mt-2">
									{API_ENDPOINT_FIX}
								</div>
							</div>
						</div>
					</div>
				)}

				<div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
					<div>
						<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">Status</div>
						<div className="flex items-center gap-2">
							<div className={classNames(
								badge[isOnline ? 'online' : 'offline'], 
								'flex-none rounded-full p-1'
							)}>
								<div className="h-1.5 w-1.5 rounded-full bg-current" />
							</div>
							<span className="text-gray-700 dark:text-zinc-200 font-medium">
								{isOnline ? 'Online' : 'Offline'}
							</span>
						</div>
					</div>
					<div>
						<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">Agent ID</div>
						<div className="text-gray-700 dark:text-zinc-200 font-mono text-sm">{agent.id}</div>
					</div>
					<div>
						<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">Hostname</div>
						<div className="text-gray-700 dark:text-zinc-200">{agent.hostname || 'N/A'}</div>
					</div>
					<div>
						<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">Connection Type</div>
						<div className="text-gray-700 dark:text-zinc-200">{agent.connection_type || 'In'}</div>
					</div>
					<div>
						<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">Last Heartbeat</div>
						<div className="text-gray-700 dark:text-zinc-200">
							{agent.last_seen 
								? new Date(agent.last_seen * 1000).toLocaleString()
								: 'Never'}
						</div>
					</div>
					<div>
						<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">Connected Since</div>
						<div className="text-gray-700 dark:text-zinc-200">
							{agent.connected_at 
								? new Date(agent.connected_at * 1000).toLocaleString()
								: 'N/A'}
						</div>
					</div>
					{agent.api_endpoint && (
						<div>
							<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">API Endpoint</div>
							<div className="text-gray-700 dark:text-zinc-200 font-mono text-xs break-all">{agent.api_endpoint}</div>
						</div>
					)}
				</div>
			</div>

			{/* System Information Card */}
			{agent.system_info && (
				<div className="bg-white dark:bg-zinc-900 border border-gray-200 dark:border-zinc-800 rounded-lg p-6 mb-6">
					<h2 className="text-lg font-semibold text-gray-700 dark:text-zinc-200 mb-4">System Information</h2>
					<div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
						<div>
							<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">Operating System</div>
							<div className="text-gray-700 dark:text-zinc-200">{agent.system_info.os_name || 'N/A'}</div>
						</div>
						<div>
							<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">OS Version</div>
							<div className="text-gray-700 dark:text-zinc-200">{agent.system_info.os_version || 'N/A'}</div>
						</div>
						<div>
							<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">Architecture</div>
							<div className="text-gray-700 dark:text-zinc-200">{agent.system_info.arch || 'N/A'}</div>
						</div>
						<div>
							<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">CPU Cores</div>
							<div className="text-gray-700 dark:text-zinc-200">{agent.system_info.cpu_count || 'N/A'}</div>
						</div>
						{agent.system_info.total_memory && (
							<div>
								<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">Total Memory</div>
								<div className="text-gray-700 dark:text-zinc-200">
									{/* sys-info returns memory in KB, formatMemory expects bytes */}
									{(() => {
										const [value, unit] = formatMemory(agent.system_info.total_memory * 1024);
										return `${value} ${unit.toUpperCase()}`;
									})()}
								</div>
							</div>
						)}
					</div>
				</div>
			)}

			{/* Resource Usage Card */}
			{hasResourceMetrics && (
				<div className="bg-white dark:bg-zinc-900 border border-gray-200 dark:border-zinc-800 rounded-lg p-6 mb-6">
					<h2 className="text-lg font-semibold text-gray-700 dark:text-zinc-200 mb-4">Resource Usage</h2>
					<div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
						{/* CPU Usage */}
						{agent.system_info.resource_usage.cpu_usage != null && (
							<div>
								<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">CPU Usage</div>
								<div className="flex items-center gap-2">
									<div className="text-gray-700 dark:text-zinc-200 font-semibold">
										{agent.system_info.resource_usage.cpu_usage.toFixed(1)}%
									</div>
									<div className="flex-1 bg-gray-100 dark:bg-zinc-800 rounded-full h-2">
										<div 
											className={classNames(
												"h-2 rounded-full transition-all",
												agent.system_info.resource_usage.cpu_usage > 80 
													? "bg-red-500" 
													: agent.system_info.resource_usage.cpu_usage > 50 
													? "bg-yellow-500" 
													: "bg-green-500"
											)}
											style={{ width: `${Math.min(agent.system_info.resource_usage.cpu_usage, 100)}%` }}
										/>
									</div>
								</div>
							</div>
						)}

						{/* Memory Usage */}
						{agent.system_info.resource_usage.memory_percent != null && (
							<div>
								<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">Memory Usage</div>
								<div className="flex items-center gap-2">
									<div className="text-gray-700 dark:text-zinc-200 font-semibold">
										{agent.system_info.resource_usage.memory_percent.toFixed(1)}%
									</div>
									<div className="flex-1 bg-gray-100 dark:bg-zinc-800 rounded-full h-2">
										<div 
											className={classNames(
												"h-2 rounded-full transition-all",
												agent.system_info.resource_usage.memory_percent > 80 
													? "bg-red-500" 
													: agent.system_info.resource_usage.memory_percent > 50 
													? "bg-yellow-500" 
													: "bg-green-500"
											)}
											style={{ width: `${Math.min(agent.system_info.resource_usage.memory_percent, 100)}%` }}
										/>
									</div>
								</div>
								{agent.system_info.resource_usage.memory_available && agent.system_info.total_memory && (
									<div className="text-xs text-gray-900 dark:text-gray-400 dark:text-zinc-500 mt-1">
										{(() => {
											const [freeValue, freeUnit] = formatMemory(agent.system_info.resource_usage.memory_available * 1024);
											const [totalValue, totalUnit] = formatMemory(agent.system_info.total_memory * 1024);
											return `${freeValue} ${freeUnit.toUpperCase()} free of ${totalValue} ${totalUnit.toUpperCase()}`;
										})()}
									</div>
								)}
							</div>
						)}

						{/* Disk Usage */}
						{agent.system_info.resource_usage.disk_percent != null && (
							<div>
								<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">Disk Usage</div>
								<div className="flex items-center gap-2">
									<div className="text-gray-700 dark:text-zinc-200 font-semibold">
										{agent.system_info.resource_usage.disk_percent.toFixed(1)}%
									</div>
									<div className="flex-1 bg-gray-100 dark:bg-zinc-800 rounded-full h-2">
										<div 
											className={classNames(
												"h-2 rounded-full transition-all",
												agent.system_info.resource_usage.disk_percent > 90 
													? "bg-red-500" 
													: agent.system_info.resource_usage.disk_percent > 70 
													? "bg-yellow-500" 
													: "bg-green-500"
											)}
											style={{ width: `${Math.min(agent.system_info.resource_usage.disk_percent, 100)}%` }}
										/>
									</div>
								</div>
								{agent.system_info.resource_usage.disk_free && agent.system_info.resource_usage.disk_total && (
									<div className="text-xs text-gray-900 dark:text-gray-400 dark:text-zinc-500 mt-1">
										{(() => {
											const [freeValue, freeUnit] = formatMemory(agent.system_info.resource_usage.disk_free * 1024);
											const [totalValue, totalUnit] = formatMemory(agent.system_info.resource_usage.disk_total * 1024);
											return `${freeValue} ${freeUnit.toUpperCase()} free of ${totalValue} ${totalUnit.toUpperCase()}`;
										})()}
									</div>
								)}
							</div>
						)}

						{/* Load Average */}
						{(agent.system_info.resource_usage.load_avg_1 != null || 
						  agent.system_info.resource_usage.load_avg_5 != null || 
						  agent.system_info.resource_usage.load_avg_15 != null) && (
							<div>
								<div className="text-sm text-gray-500 dark:text-zinc-400 mb-1">Load Average</div>
								<div className="text-gray-700 dark:text-zinc-200">
									<span className="font-semibold">{agent.system_info.resource_usage.load_avg_1?.toFixed(2) ?? '?'}</span>
									{' / '}
									<span>{agent.system_info.resource_usage.load_avg_5?.toFixed(2) ?? '?'}</span>
									{' / '}
									<span>{agent.system_info.resource_usage.load_avg_15?.toFixed(2) ?? '?'}</span>
								</div>
								<div className="text-xs text-gray-900 dark:text-gray-400 dark:text-zinc-500 mt-1">1 / 5 / 15 min</div>
							</div>
						)}
					</div>
				</div>
			)}

			{/* Processes Section */}
			<div className="bg-white dark:bg-zinc-900 border border-gray-200 dark:border-zinc-800 rounded-lg p-6">
				<h2 className="text-lg font-semibold text-gray-700 dark:text-zinc-200 mb-4">
					Processes ({processes.length})
				</h2>
				
				{processes.length === 0 ? (
					<div className="text-center py-8">
						<div className="text-gray-500 dark:text-zinc-400">No processes running on this agent</div>
						<div className="text-gray-900 dark:text-gray-400 dark:text-zinc-500 text-sm mt-2">
							Processes started via this agent will appear here
						</div>
					</div>
				) : (
					<table className="w-full whitespace-nowrap text-left">
						<thead className="border-b border-gray-200 dark:border-zinc-800 text-sm leading-6 text-gray-500 dark:text-zinc-400">
							<tr>
								<th scope="col" className="py-2 pl-4 pr-8 font-semibold">
									Name
								</th>
								<th scope="col" className="hidden py-2 pl-0 pr-8 font-semibold sm:table-cell">
									PID
								</th>
								<th scope="col" className="hidden py-2 pl-0 pr-8 font-semibold sm:table-cell">
									Status
								</th>
								<th scope="col" className="hidden py-2 pl-0 pr-8 font-semibold md:table-cell">
									CPU
								</th>
								<th scope="col" className="hidden py-2 pl-0 pr-8 font-semibold md:table-cell">
									Memory
								</th>
								<th scope="col" className="py-2 pl-0 pr-4 text-right font-semibold">
									Uptime
								</th>
							</tr>
						</thead>
						<tbody className="divide-y divide-zinc-800">
							{processes.map((process: any) => (
								<tr key={process.id} className="hover:bg-gray-100 dark:bg-zinc-800/30 transition">
									<td className="py-3 pl-4 pr-8">
										<div className="text-sm font-medium text-white">{process.name}</div>
									</td>
									<td className="hidden py-3 pl-0 pr-8 sm:table-cell">
										<div className="text-sm text-gray-500 dark:text-zinc-400 font-mono">{process.pid || 'N/A'}</div>
									</td>
									<td className="hidden py-3 pl-0 pr-8 sm:table-cell">
										<div className={classNames(
											process.running ? badge.running : badge.stopped,
											'inline-flex rounded-md px-2 py-1 text-xs font-medium ring-1 ring-inset ring-white/10'
										)}>
											{process.running ? 'Running' : 'Stopped'}
										</div>
									</td>
									<td className="hidden py-3 pl-0 pr-8 md:table-cell">
										<div className="text-sm text-gray-500 dark:text-zinc-400">
											{process.cpu || 'N/A'}
										</div>
									</td>
									<td className="hidden py-3 pl-0 pr-8 md:table-cell">
										<div className="text-sm text-gray-500 dark:text-zinc-400">
											{process.memory ? process.memory.toUpperCase() : 'N/A'}
										</div>
									</td>
									<td className="py-3 pl-0 pr-4 text-right">
										<div className="text-sm text-gray-500 dark:text-zinc-400">
											{process.uptime ? startDuration(process.uptime, false) : 'N/A'}
										</div>
									</td>
								</tr>
							))}
						</tbody>
					</table>
				)}
			</div>
		</Fragment>
	);
};

export default AgentDetail;
