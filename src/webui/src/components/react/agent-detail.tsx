import { api } from '@/api';
import { useEffect, Fragment, useState } from 'react';
import Loader from '@/components/react/loader';
import Header from '@/components/react/header';
import { useArray, classNames, startDuration, formatMemory, isLocalAgent } from '@/helpers';

const AgentDetail = (props: { agentId: string; base: string }) => {
	const [agent, setAgent] = useState<any>(null);
	const [processes, setProcesses] = useState<any[]>([]);
	const [loading, setLoading] = useState(true);
	const [error, setError] = useState<string | null>(null);

	// Fix instruction for API endpoint issues
	const API_ENDPOINT_FIX = (
		<>
			<strong>To fix:</strong> Restart the agent with:{' '}
			<code className="bg-zinc-800 px-1 py-0.5 rounded">
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

	async function fetchAgentDetails() {
		setLoading(true);
		setError(null);
		try {
			// Fetch agent info
			const agentResponse = await api.get(`${props.base}/daemon/agents/${props.agentId}`).json();
			setAgent(agentResponse);

			// Fetch processes for this agent
			try {
				const processesResponse = await api.get(`${props.base}/daemon/agents/${props.agentId}/processes`).json();
				setProcesses(Array.isArray(processesResponse) ? processesResponse : []);
			} catch (e) {
				console.warn('Failed to fetch agent processes:', e);
				// If endpoint fails, set empty array (agent might not have any processes)
				setProcesses([]);
			}
		} catch (error) {
			const err = error as any;
			console.error('Failed to fetch agent details:', error);
			
			// Provide more specific error messages
			// ky HTTPError has response property with status code
			if (err.response?.status) {
				const status = err.response.status;
				if (status === 404) {
					setError('Agent not found. The agent may have disconnected or never connected to this server.');
				} else if (status === 401 || status === 403) {
					setError('Unauthorized. Please check your API token configuration.');
				} else {
					setError(`Server error (${status}): ${err.message || 'Failed to load agent details'}`);
				}
			} else if (err.message) {
				setError(err.message);
			} else {
				setError('Failed to load agent details. Please check your connection and try again.');
			}
		} finally {
			setLoading(false);
		}
	}

	useEffect(() => {
		fetchAgentDetails();
		// Auto-refresh every 5 seconds
		const interval = setInterval(fetchAgentDetails, 5000);
		return () => clearInterval(interval);
	}, [props.agentId]);

	if (loading) {
		return (
			<div className="min-h-screen flex items-center justify-center">
				<Loader />
			</div>
		);
	}

	if (error || !agent) {
		return (
			<Fragment>
				<Header name="Agent Details" description="Detailed information about this agent and its processes.">
					<div className="flex gap-2">
						<button
							type="button"
							onClick={fetchAgentDetails}
							className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-900 hover:border-zinc-800 bg-zinc-950 text-zinc-50 hover:bg-zinc-900 px-4 py-2 text-sm font-semibold rounded-lg">
							Retry
						</button>
						<a
							href={`${props.base}/servers`}
							className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-700 hover:border-zinc-600 bg-zinc-800 text-zinc-50 hover:bg-zinc-700 px-4 py-2 text-sm font-semibold rounded-lg">
							Back to Agents
						</a>
					</div>
				</Header>
				<div className="bg-zinc-900 border border-zinc-800 rounded-lg p-6">
					<div className="text-center py-12 px-4">
						<div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-red-500/10 mb-4">
							<svg className="w-8 h-8 text-red-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
								<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
							</svg>
						</div>
						<div className="text-red-400 text-xl font-semibold mb-3">Failed to Load Agent Details</div>
						<div className="text-zinc-300 text-base mb-6 max-w-md mx-auto">
							{error || 'Agent not found or connection failed'}
						</div>
						<div className="text-zinc-500 text-sm">
							<p className="mb-2">Agent ID: <code className="bg-zinc-800 px-2 py-1 rounded text-zinc-300">{props.agentId}</code></p>
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

	return (
		<Fragment>
			<Header name={`Agent: ${agent.name}`} description="Detailed information about this agent and its processes.">
				<div className="flex gap-2">
					<button
						type="button"
						onClick={fetchAgentDetails}
						className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-900 hover:border-zinc-800 bg-zinc-950 text-zinc-50 hover:bg-zinc-900 px-4 py-2 text-sm font-semibold rounded-lg">
						Refresh
					</button>
					<a
						href={`${props.base}/servers`}
						className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-700 hover:border-zinc-600 bg-zinc-800 text-zinc-50 hover:bg-zinc-700 px-4 py-2 text-sm font-semibold rounded-lg">
						Back to Agents
					</a>
				</div>
			</Header>

			{/* Agent Information Card */}
			<div className="bg-zinc-900 border border-zinc-800 rounded-lg p-6 mb-6">
				<h2 className="text-lg font-semibold text-zinc-200 mb-4">Agent Information</h2>
				
				{/* Warning if agent doesn't have API endpoint */}
				{!agent.api_endpoint && !isLocalAgent(agent) && (
					<div className="mb-4 p-3 bg-amber-900/20 border border-amber-700/50 rounded-lg">
						<div className="flex items-start gap-3">
							<svg className="w-5 h-5 text-amber-400 mt-0.5 flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24">
								<path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
							</svg>
							<div>
								<div className="text-amber-400 font-medium text-sm">Limited Functionality - No API Endpoint</div>
								<div className="text-zinc-300 text-sm mt-1">
									This agent doesn't have an accessible API endpoint configured. Process management actions (start, stop, restart) will not be available.
								</div>
								<div className="text-zinc-400 text-xs mt-2">
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
								<div className="text-zinc-300 text-sm mt-1">
									This agent's API endpoint uses "localhost" which is not accessible from the server. Process management actions will fail.
								</div>
								<div className="text-zinc-400 text-xs mt-2">
									{API_ENDPOINT_FIX}
								</div>
							</div>
						</div>
					</div>
				)}

				<div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
					<div>
						<div className="text-sm text-zinc-400 mb-1">Status</div>
						<div className="flex items-center gap-2">
							<div className={classNames(
								badge[isOnline ? 'online' : 'offline'], 
								'flex-none rounded-full p-1'
							)}>
								<div className="h-1.5 w-1.5 rounded-full bg-current" />
							</div>
							<span className="text-zinc-200 font-medium">
								{isOnline ? 'Online' : 'Offline'}
							</span>
						</div>
					</div>
					<div>
						<div className="text-sm text-zinc-400 mb-1">Agent ID</div>
						<div className="text-zinc-200 font-mono text-sm">{agent.id}</div>
					</div>
					<div>
						<div className="text-sm text-zinc-400 mb-1">Hostname</div>
						<div className="text-zinc-200">{agent.hostname || 'N/A'}</div>
					</div>
					<div>
						<div className="text-sm text-zinc-400 mb-1">Connection Type</div>
						<div className="text-zinc-200">{agent.connection_type || 'In'}</div>
					</div>
					<div>
						<div className="text-sm text-zinc-400 mb-1">Last Heartbeat</div>
						<div className="text-zinc-200">
							{agent.last_seen 
								? new Date(agent.last_seen * 1000).toLocaleString()
								: 'Never'}
						</div>
					</div>
					<div>
						<div className="text-sm text-zinc-400 mb-1">Connected Since</div>
						<div className="text-zinc-200">
							{agent.connected_at 
								? new Date(agent.connected_at * 1000).toLocaleString()
								: 'N/A'}
						</div>
					</div>
					{agent.api_endpoint && (
						<div>
							<div className="text-sm text-zinc-400 mb-1">API Endpoint</div>
							<div className="text-zinc-200 font-mono text-xs break-all">{agent.api_endpoint}</div>
						</div>
					)}
				</div>
			</div>

			{/* System Information Card */}
			{agent.system_info && (
				<div className="bg-zinc-900 border border-zinc-800 rounded-lg p-6 mb-6">
					<h2 className="text-lg font-semibold text-zinc-200 mb-4">System Information</h2>
					<div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
						<div>
							<div className="text-sm text-zinc-400 mb-1">Operating System</div>
							<div className="text-zinc-200">{agent.system_info.os_name || 'N/A'}</div>
						</div>
						<div>
							<div className="text-sm text-zinc-400 mb-1">OS Version</div>
							<div className="text-zinc-200">{agent.system_info.os_version || 'N/A'}</div>
						</div>
						<div>
							<div className="text-sm text-zinc-400 mb-1">Architecture</div>
							<div className="text-zinc-200">{agent.system_info.arch || 'N/A'}</div>
						</div>
						<div>
							<div className="text-sm text-zinc-400 mb-1">CPU Cores</div>
							<div className="text-zinc-200">{agent.system_info.cpu_count || 'N/A'}</div>
						</div>
						{agent.system_info.total_memory && (
							<div>
								<div className="text-sm text-zinc-400 mb-1">Total Memory</div>
								<div className="text-zinc-200">
									{/* sys-info returns memory in KB, formatMemory expects bytes */}
									{formatMemory(agent.system_info.total_memory * 1024)}
								</div>
							</div>
						)}
					</div>
				</div>
			)}

			{/* Resource Usage Card */}
			{agent.system_info?.resource_usage && (
				<div className="bg-zinc-900 border border-zinc-800 rounded-lg p-6 mb-6">
					<h2 className="text-lg font-semibold text-zinc-200 mb-4">Resource Usage</h2>
					<div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
						{/* CPU Usage */}
						{agent.system_info.resource_usage.cpu_usage != null && (
							<div>
								<div className="text-sm text-zinc-400 mb-1">CPU Usage</div>
								<div className="flex items-center gap-2">
									<div className="text-zinc-200 font-semibold">
										{agent.system_info.resource_usage.cpu_usage.toFixed(1)}%
									</div>
									<div className="flex-1 bg-zinc-800 rounded-full h-2">
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
								<div className="text-sm text-zinc-400 mb-1">Memory Usage</div>
								<div className="flex items-center gap-2">
									<div className="text-zinc-200 font-semibold">
										{agent.system_info.resource_usage.memory_percent.toFixed(1)}%
									</div>
									<div className="flex-1 bg-zinc-800 rounded-full h-2">
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
								{agent.system_info.resource_usage.memory_used && (
									<div className="text-xs text-zinc-500 mt-1">
										{formatMemory(agent.system_info.resource_usage.memory_used * 1024)} used
									</div>
								)}
							</div>
						)}

						{/* Disk Usage */}
						{agent.system_info.resource_usage.disk_percent != null && (
							<div>
								<div className="text-sm text-zinc-400 mb-1">Disk Usage</div>
								<div className="flex items-center gap-2">
									<div className="text-zinc-200 font-semibold">
										{agent.system_info.resource_usage.disk_percent.toFixed(1)}%
									</div>
									<div className="flex-1 bg-zinc-800 rounded-full h-2">
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
									<div className="text-xs text-zinc-500 mt-1">
										{formatMemory(agent.system_info.resource_usage.disk_free * 1024)} free of {formatMemory(agent.system_info.resource_usage.disk_total * 1024)}
									</div>
								)}
							</div>
						)}

						{/* Load Average */}
						{(agent.system_info.resource_usage.load_avg_1 != null || 
						  agent.system_info.resource_usage.load_avg_5 != null || 
						  agent.system_info.resource_usage.load_avg_15 != null) && (
							<div>
								<div className="text-sm text-zinc-400 mb-1">Load Average</div>
								<div className="text-zinc-200">
									<span className="font-semibold">{agent.system_info.resource_usage.load_avg_1?.toFixed(2) ?? '?'}</span>
									{' / '}
									<span>{agent.system_info.resource_usage.load_avg_5?.toFixed(2) ?? '?'}</span>
									{' / '}
									<span>{agent.system_info.resource_usage.load_avg_15?.toFixed(2) ?? '?'}</span>
								</div>
								<div className="text-xs text-zinc-500 mt-1">1 / 5 / 15 min</div>
							</div>
						)}
					</div>
				</div>
			)}

			{/* Processes Section */}
			<div className="bg-zinc-900 border border-zinc-800 rounded-lg p-6">
				<h2 className="text-lg font-semibold text-zinc-200 mb-4">
					Processes ({processes.length})
				</h2>
				
				{processes.length === 0 ? (
					<div className="text-center py-8">
						<div className="text-zinc-400">No processes running on this agent</div>
						<div className="text-zinc-500 text-sm mt-2">
							Processes started via this agent will appear here
						</div>
					</div>
				) : (
					<table className="w-full whitespace-nowrap text-left">
						<thead className="border-b border-zinc-800 text-sm leading-6 text-zinc-400">
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
								<tr key={process.id} className="hover:bg-zinc-800/30 transition">
									<td className="py-3 pl-4 pr-8">
										<div className="text-sm font-medium text-white">{process.name}</div>
									</td>
									<td className="hidden py-3 pl-0 pr-8 sm:table-cell">
										<div className="text-sm text-zinc-400 font-mono">{process.pid || 'N/A'}</div>
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
										<div className="text-sm text-zinc-400">
											{process.cpu ? `${process.cpu.toFixed(1)}%` : 'N/A'}
										</div>
									</td>
									<td className="hidden py-3 pl-0 pr-8 md:table-cell">
										<div className="text-sm text-zinc-400">
											{process.memory ? formatMemory(process.memory) : 'N/A'}
										</div>
									</td>
									<td className="py-3 pl-0 pr-4 text-right">
										<div className="text-sm text-zinc-400">
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
