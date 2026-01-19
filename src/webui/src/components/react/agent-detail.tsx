import { api } from '@/api';
import { useEffect, Fragment, useState } from 'react';
import Loader from '@/components/react/loader';
import Header from '@/components/react/header';
import { useArray, classNames, startDuration, formatMemory } from '@/helpers';

const AgentDetail = (props: { agentId: string; base: string }) => {
	const [agent, setAgent] = useState<any>(null);
	const [processes, setProcesses] = useState<any[]>([]);
	const [loading, setLoading] = useState(true);
	const [error, setError] = useState<string | null>(null);

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
				// If endpoint doesn't exist yet, set empty array
				setProcesses([]);
			}
		} catch (error) {
			console.error('Failed to fetch agent details:', error);
			setError((error as Error).message || 'Failed to load agent details');
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
		return <Loader />;
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
					<div className="text-center py-8">
						<div className="text-red-400 text-lg mb-4">Failed to load agent details</div>
						<div className="text-zinc-400 text-sm">
							{error || 'Agent not found or connection failed'}
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
				</div>
			</div>

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
