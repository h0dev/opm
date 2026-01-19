import { api } from '@/api';
import { useEffect, Fragment, useState } from 'react';
import Loader from '@/components/react/loader';
import Header from '@/components/react/header';
import { useArray, classNames, startDuration, isLocalAgent } from '@/helpers';
import ToastContainer from '@/components/react/toast';
import { useToast } from '@/components/react/useToast';

const Index = (props: { base: string }) => {
	const { toasts, closeToast, error } = useToast();
	const agents = useArray([]);
	const [loading, setLoading] = useState(true);
	const [serverHost, setServerHost] = useState('localhost');
	const [retryTrigger, setRetryTrigger] = useState(0);

	const badge = {
		online: 'bg-emerald-400/10 text-emerald-400',
		offline: 'bg-red-500/10 text-red-500'
	};

	// Auto-detect server host from current URL
	useEffect(() => {
		const hostname = window.location.hostname;
		const port = window.location.port || '9876';
		setServerHost(`${hostname}:${port}`);
	}, []);

	const handleRefresh = () => {
		setLoading(true);
		setRetryTrigger(prev => prev + 1);
	};

	const removeAgent = async (agentId: string, agentName: string) => {
		if (!confirm(`Are you sure you want to remove agent "${agentName}"?`)) return;

		try {
			await api.delete(`${props.base}/daemon/agents/${agentId}`);
			// Agent list will automatically update via SSE
		} catch (err) {
			error('Failed to remove agent: ' + (err as Error).message);
		}
	};

	useEffect(() => {
		// Use Server-Sent Events for real-time updates instead of polling
		setLoading(true);
		
		const eventSource = new EventSource(`${props.base}/live/agents`);
		
		eventSource.onmessage = (event) => {
			try {
				const data = JSON.parse(event.data);
				agents.clear();
				if (Array.isArray(data)) {
					data.forEach((agent: any) => agents.push(agent));
				}
				setLoading(false);
			} catch (err) {
				console.error('Failed to parse SSE data:', err);
				setLoading(false);
			}
		};
		
		eventSource.onerror = (err) => {
			console.error('SSE connection error:', err);
			setLoading(false);
			// EventSource will automatically reconnect
		};
		
		return () => {
			eventSource.close();
		};
	}, [retryTrigger]);

	if (loading) {
		return <Loader />;
	}

	return (
		<Fragment>
			<ToastContainer toasts={toasts} onClose={closeToast} />
			<Header name="Connected Agents" description="A list of all agents connected to this server.">
				<div className="flex gap-2">
					<button
						type="button"
						onClick={handleRefresh}
						className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-900 hover:border-zinc-800 bg-zinc-950 text-zinc-50 hover:bg-zinc-900 px-4 py-2 text-sm font-semibold rounded-lg">
						Refresh
					</button>
				</div>
			</Header>

			{agents.isEmpty() ? (
				<div className="text-center py-12">
					<div className="text-zinc-400 text-lg mb-4">No agents connected</div>
					<div className="text-zinc-500 text-sm space-y-2 max-w-2xl mx-auto">
						<p>To connect an agent to this server, run the following command on a remote machine:</p>
						<code className="block bg-zinc-900 border border-zinc-800 rounded-lg px-4 py-3 text-left text-zinc-300 font-mono text-sm mt-4">
							opm agent connect http://{serverHost} --name my-agent
						</code>
						<p className="mt-4 text-xs text-zinc-400">
							Replace the hostname if needed. The command is automatically generated based on your current connection.
						</p>
					</div>
				</div>
			) : (
				<table className="w-full whitespace-nowrap text-left">
					<colgroup>
						<col className="w-full sm:w-3/12" />
						<col className="lg:w-2/12" />
						<col className="lg:w-2/12" />
						<col className="lg:w-2/12" />
						<col className="lg:w-2/12" />
						<col className="lg:w-1/12" />
						<col className="lg:w-1/12" />
					</colgroup>
					<thead className="sticky top-0 z-10 bg-zinc-950 bg-opacity-75 backdrop-blur backdrop-filter border-b border-white/10 text-sm leading-6 text-white">
						<tr>
							<th scope="col" className="py-2 pl-4 pr-8 font-semibold sm:pl-6 lg:pl-8">
								Agent Name
							</th>
							<th scope="col" className="hidden py-2 pl-0 pr-8 font-semibold sm:table-cell">
								Hostname
							</th>
							<th scope="col" className="hidden py-2 pl-0 pr-8 font-semibold sm:table-cell">
								Agent ID
							</th>
							<th scope="col" className="hidden py-2 pl-0 pr-8 font-semibold sm:table-cell">
								Type
							</th>
							<th scope="col" className="hidden py-2 pl-0 pr-8 font-semibold md:table-cell lg:pr-20">
								Last Heartbeat
							</th>
							<th scope="col" className="py-2 pl-0 pr-4 font-semibold sm:table-cell sm:pr-6 lg:pr-8">
								Status
							</th>
							<th scope="col" className="py-2 pl-0 pr-4 text-right font-semibold sm:pr-6 lg:pr-8">
								Actions
							</th>
						</tr>
					</thead>
					<tbody className="divide-y divide-white/5 border-b border-white/5">
						{agents.value.map((agent: any) => {
							// Backend sends last_seen as seconds since UNIX epoch
							// Heartbeat interval is 30s by default, so we use 60s threshold (2x) to account for network delays
							const isOnline = agent.last_seen && 
								(Date.now() - agent.last_seen * 1000) < 60000; // 60 seconds threshold (2x 30s heartbeat interval)
							
							return (
								<tr 
									key={agent.id} 
									className="hover:bg-zinc-800/30 transition cursor-pointer"
									onClick={() => window.location.href = `${props.base}/agent-detail#${agent.id}`}
								>
									<td className="py-4 pl-4 pr-8 sm:pl-6 lg:pl-8">
										<div className="flex items-center gap-x-4">
											<div className={classNames(
												isOnline ? 'bg-emerald-400' : 'bg-red-500',
												'h-2 w-2 rounded-full'
											)} />
											<div className="truncate text-sm font-medium leading-6 text-white">
												{agent.name}
											</div>
										</div>
									</td>
									<td className="hidden py-4 pl-0 pr-4 sm:table-cell sm:pr-8">
										<div className="text-sm leading-6 text-gray-400">
											{agent.hostname || 'N/A'}
										</div>
									</td>
									<td className="hidden py-4 pl-0 pr-4 sm:table-cell sm:pr-8">
										<div className="font-mono text-xs leading-6 text-gray-400">
											{agent.id ? agent.id.slice(0, 8) + '...' : 'N/A'}
										</div>
									</td>
									<td className="hidden py-4 pl-0 pr-4 sm:table-cell sm:pr-8">
										<div className="text-sm leading-6 text-gray-400">
											{agent.connection_type || 'In'}
										</div>
									</td>
									<td className="hidden py-4 pl-0 pr-8 text-sm leading-6 text-gray-400 md:table-cell lg:pr-20">
										{agent.last_seen 
											? new Date(agent.last_seen * 1000).toLocaleString()
											: 'Never'}
									</td>
									<td className="py-4 pl-0 pr-4 sm:pr-8 lg:pr-20">
										<div className="flex items-center justify-end gap-x-2 sm:justify-start">
											<div className={classNames(
												badge[isOnline ? 'online' : 'offline'], 
												'flex-none rounded-full p-1'
											)}>
												<div className="h-1.5 w-1.5 rounded-full bg-current" />
											</div>
											<div className="text-white">
												{isOnline ? 'Online' : 'Offline'}
											</div>
										</div>
									</td>
									<td className="py-4 pl-0 pr-4 text-right sm:pr-6 lg:pr-8" onClick={(e) => e.stopPropagation()}>
										{!isLocalAgent(agent) && (
											<button
												onClick={() => removeAgent(agent.id, agent.name)}
												className="text-red-400 hover:text-red-300 text-sm font-medium transition">
												Remove
											</button>
										)}
									</td>
								</tr>
							);
						})}
					</tbody>
				</table>
			)}
		</Fragment>
	);
};

export default Index;
