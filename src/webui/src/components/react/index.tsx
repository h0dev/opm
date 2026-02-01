import { api } from '@/api';
import InlineRename from '@/components/react/inline-rename';
import Loader from '@/components/react/loader';
import Header from '@/components/react/header';
import { useArray, classNames } from '@/helpers';
import { useEffect, useState, Fragment, useRef, useMemo } from 'react';
import { EllipsisVerticalIcon } from '@heroicons/react/20/solid';
import { Menu, MenuItem, MenuItems, MenuButton, Transition } from '@headlessui/react';
import ToastContainer from '@/components/react/toast';
import { useToast } from '@/components/react/useToast';
import { ACTION_MESSAGES } from '@/constants';

// Delay in milliseconds to wait for agent ProcessUpdate messages to arrive at server
// before refreshing the UI. This ensures the UI shows the updated process state.
const AGENT_PROCESS_UPDATE_DELAY_MS = 200;

type ProcessItem = {
	id: number;
	name: string;
	server: string;
	status: string;
	pid: string;
	uptime: string;
	restarts: number;
	cpu: string;
	mem: string;
	watch: string;
	agent_id?: string;
	agent_name?: string;
	agent_api_endpoint?: string;
};

const Index = (props: { base: string }) => {
	const { toasts, closeToast, success, error } = useToast();
	const items = useArray<ProcessItem>([]);
	const [searchTerm, setSearchTerm] = useState('');
	const [statusFilter, setStatusFilter] = useState('all');
	const [agentFilter, setAgentFilter] = useState('all');
	const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
	const [showBulkActions, setShowBulkActions] = useState(false);
	const [loading, setLoading] = useState(true);
	const renameRefs = useRef<Map<number, { triggerEdit: () => void }>>(new Map());

	const badge = {
		online: 'bg-emerald-400',
		stopped: 'bg-red-500',
		crashed: 'bg-amber-400'
	};

	async function fetch() {
		try {
			items.clear();
			setSelectedIds(new Set()); // Clear selections on refresh

			const res = await api.get(props.base + '/list').json();
			res.map((s) => items.push({ ...s, server: 'local' }));

			try {
				const servers = await api.get(props.base + '/daemon/servers').json();
				// Use Promise.all for parallel fetching instead of sequential forEach
				const remotePromises = servers.map(async (name) => {
					try {
						const remote = await api.get(props.base + `/remote/${name}/list`).json();
						return remote.map((s) => ({ ...s, server: name }));
					} catch (err) {
						console.error(`Failed to fetch processes from remote ${name}:`, err);
						return [];
					}
				});
				const remoteResults = await Promise.all(remotePromises);
				remoteResults.flat().forEach((item) => items.push(item));
			} catch {}
		} finally {
			setLoading(false);
		}
	}

	// Component for displaying process details
	const ProcessDetails = ({ item, isClickable = true }: { item: ProcessItem; isClickable?: boolean }) => {
		return (
			<dl className="-my-3 divide-y divide-zinc-800/30 px-6 py-4 text-sm leading-6">
				<div className={`flex justify-between gap-x-2 py-2 ${isClickable ? 'transition-colors hover:text-gray-600 dark:text-zinc-300' : ''}`}>
					<dt className="text-zinc-600 font-medium">cpu usage</dt>
					<dd className="text-gray-500 dark:text-zinc-400 font-mono">{isRunning(item.status) ? item.cpu : 'offline'}</dd>
				</div>
				<div className={`flex justify-between gap-x-2 py-2 ${isClickable ? 'transition-colors hover:text-gray-600 dark:text-zinc-300' : ''}`}>
					<dt className="text-zinc-600 font-medium">memory</dt>
					<dd className="text-gray-500 dark:text-zinc-400 font-mono">{isRunning(item.status) ? item.mem : 'offline'}</dd>
				</div>
				<div className={`flex justify-between gap-x-2 py-2 ${isClickable ? 'transition-colors hover:text-gray-600 dark:text-zinc-300' : ''}`}>
					<dt className="text-zinc-600 font-medium">pid</dt>
					<dd className="text-gray-500 dark:text-zinc-400 font-mono">{isRunning(item.status) ? item.pid : 'none'}</dd>
				</div>
				<div className={`flex justify-between gap-x-2 py-2 ${isClickable ? 'transition-colors hover:text-gray-600 dark:text-zinc-300' : ''}`}>
					<dt className="text-zinc-600 font-medium">uptime</dt>
					<dd className="text-gray-500 dark:text-zinc-400 font-mono">{isRunning(item.status) ? item.uptime : 'none'}</dd>
				</div>
				<div className={`flex justify-between gap-x-2 py-2 ${isClickable ? 'transition-colors hover:text-gray-600 dark:text-zinc-300' : ''}`}>
					<dt className="text-zinc-600 font-medium">restarts</dt>
					<dd className="text-gray-500 dark:text-zinc-400 font-mono">{item.restarts == 0 ? 'none' : item.restarts}</dd>
				</div>
			</dl>
		);
	};

	const getActionEndpoint = (item: ProcessItem): string => {
		// If process has agent_id, use the agent action proxy endpoint
		if (item.agent_id) {
			if (import.meta.env.DEV) {
				console.log(`Using agent proxy endpoint for process ${item.name}, agent: ${item.agent_id}`);
			}
			return `${props.base}/daemon/agents/${item.agent_id}/process/${item.id}/action`;
		}
		// Otherwise, use server-based routing (local or remote server)
		if (item.server === 'local') {
			if (import.meta.env.DEV) {
				console.log(`Using local endpoint for process ${item.name}`);
			}
			return `${props.base}/process/${item.id}/action`;
		}
		if (import.meta.env.DEV) {
			console.log(`Using remote server endpoint for process ${item.name}:`, item.server);
		}
		return `${props.base}/remote/${item.server}/action/${item.id}`;
	};

	const isRemote = (item: ProcessItem): boolean => item.server !== 'local';
	const isRunning = (status: string): boolean => !['stopped', 'crashed'].includes(status);
	
	const action = async (item: ProcessItem, name: string) => {
		const endpoint = getActionEndpoint(item);
		
		try {
			await api.post(endpoint, { json: { method: name } });
			// For agent processes, add a small delay to allow the agent to send the ProcessUpdate
			// before we refresh the UI. This ensures the UI shows the updated state.
			if (item.agent_id) {
				await new Promise(resolve => setTimeout(resolve, AGENT_PROCESS_UPDATE_DELAY_MS));
			}
			await fetch();
			success(ACTION_MESSAGES[name] || `${name} action completed successfully`);
		} catch (err) {
			error(`Failed to ${name} process: ${(err as Error).message}`);
		}
	};
	
	// Toggle selection
	const toggleSelect = (id: number) => {
		const newSelected = new Set(selectedIds);
		if (newSelected.has(id)) {
			newSelected.delete(id);
		} else {
			newSelected.add(id);
		}
		setSelectedIds(newSelected);
		setShowBulkActions(newSelected.size > 0);
	};

	// Select all visible items
	const selectAll = () => {
		const allIds = new Set(filteredItems.map((item) => item.id));
		setSelectedIds(allIds);
		setShowBulkActions(allIds.size > 0);
	};

	// Clear selection
	const clearSelection = () => {
		setSelectedIds(new Set());
		setShowBulkActions(false);
	};

	// Bulk action
	const bulkAction = async (method: string) => {
		if (selectedIds.size === 0) return;
		
		try {
			// Group selected items by server
			const selectedItems = items.value.filter((item) => selectedIds.has(item.id));
			const groupedByServer = selectedItems.reduce((acc, item) => {
				const server = item.server || 'local';
				if (!acc[server]) {
					acc[server] = [];
				}
				acc[server].push(item.id);
				return acc;
			}, {} as Record<string, number[]>);

			// Check if any agent processes are involved
			const hasAgentProcesses = selectedItems.some(item => item.agent_id);

			// Execute actions for each server group
			const promises = Object.entries(groupedByServer).map(([server, ids]) => {
				if (server === 'local') {
					return api.post(`${props.base}/process/bulk-action`, {
						json: { ids, method }
					});
				} else {
					// For remote servers, we need to call actions individually
					// since there's no bulk-action endpoint for remote servers
					return Promise.all(
						ids.map(id => 
							api.post(`${props.base}/remote/${server}/action/${id}`, {
								json: { method }
							})
						)
					);
				}
			});

			await Promise.all(promises);
			// For agent processes, add a delay to allow ProcessUpdate messages to arrive
			if (hasAgentProcesses) {
				await new Promise(resolve => setTimeout(resolve, AGENT_PROCESS_UPDATE_DELAY_MS));
			}
			await fetch();
			success(`${method} action completed on ${selectedIds.size} processes`);
		} catch (err) {
			error('Failed to perform bulk action: ' + (err as Error).message);
		}
	};
	
	// Save all processes
	const saveAll = async () => {
		try {
			await api.post(`${props.base}/daemon/save`, {});
			success('All processes saved to dumpfile');
		} catch (err) {
			error('Failed to save processes: ' + (err as Error).message);
		}
	};
	
	// Restore all processes
	const restoreAll = async () => {
		try {
			await api.post(`${props.base}/daemon/restore`, {});
			fetch();
			success('All processes restored from dumpfile');
		} catch (err) {
			error('Failed to restore processes: ' + (err as Error).message);
		}
	};

	// Filter items based on search term, status filter, and agent filter
	const filteredItems = items.value.filter((item) => {
		const matchesSearch = searchTerm === '' || 
			item.name.toLowerCase().includes(searchTerm.toLowerCase()) ||
			item.server.toLowerCase().includes(searchTerm.toLowerCase()) ||
			(item.agent_name && item.agent_name.toLowerCase().includes(searchTerm.toLowerCase()));
		
		const matchesStatus = statusFilter === 'all' || item.status === statusFilter;
		
		const matchesAgent = agentFilter === 'all' || 
			(agentFilter === 'local' && !item.agent_id) ||
			item.agent_id === agentFilter;
		
		return matchesSearch && matchesStatus && matchesAgent;
	});

	// Get unique agent IDs and names for the filter dropdown
	const agents = useMemo(() => {
		const agentMap = new Map<string, { id: string; name: string }>();
		items.value.forEach(item => {
			if (item.agent_id && !agentMap.has(item.agent_id)) {
				agentMap.set(item.agent_id, {
					id: item.agent_id,
					name: item.agent_name || item.agent_id
				});
			}
		});
		return Array.from(agentMap.values());
	}, [items.value]);

	useEffect(() => {
		fetch();
	}, []);

	// Auto-refresh every 5 seconds
	// Note: Empty dependency array is correct - fetch doesn't depend on state/props that change
	useEffect(() => {
		const intervalId = setInterval(() => {
			fetch();
		}, 5000); // 5 seconds

		return () => clearInterval(intervalId);
	}, []);

	if (loading) {
		return <Loader />;
	}

	if (items.isEmpty()) {
		return (
			<Fragment>
				<ToastContainer toasts={toasts} onClose={closeToast} />
				<Header name="No processes running" description="Start managing your processes with OPM.">
					<div className="flex gap-2">
						<button
							type="button"
							onClick={saveAll}
							className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-gray-300 dark:border-zinc-700 hover:border-zinc-600 bg-gray-100 dark:bg-zinc-800 text-gray-900 dark:text-zinc-50 hover:bg-gray-200 dark:bg-zinc-700 px-3 py-2 text-sm font-semibold rounded-lg">
							Save
						</button>
						<button
							type="button"
							onClick={restoreAll}
							className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-gray-300 dark:border-zinc-700 hover:border-zinc-600 bg-gray-100 dark:bg-zinc-800 text-gray-900 dark:text-zinc-50 hover:bg-gray-200 dark:bg-zinc-700 px-3 py-2 text-sm font-semibold rounded-lg">
							Restore
						</button>
						<button
							type="button"
							onClick={fetch}
							className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-900 hover:border-gray-200 dark:border-zinc-800 bg-zinc-950 text-white dark:text-zinc-50 hover:bg-white hover:text-gray-900 dark:hover:bg-zinc-900 px-4 py-2 text-sm font-semibold rounded-lg">
							Refresh
						</button>
					</div>
				</Header>
				<div className="text-center py-12 px-4">
					<div className="text-gray-500 dark:text-zinc-400 text-lg mb-4">No processes are currently running</div>
					<div className="text-gray-900 dark:text-gray-400 dark:text-zinc-500 text-sm space-y-2 max-w-2xl mx-auto">
						<p>Start a new process using the OPM CLI:</p>
						<code className="block bg-white dark:bg-zinc-900 border border-gray-200 dark:border-zinc-800 rounded-lg px-4 py-3 text-left text-gray-600 dark:text-zinc-300 font-mono text-sm mt-4">
							opm start &lt;script&gt; --name &lt;name&gt;
						</code>
						<p className="mt-4 text-xs text-gray-500 dark:text-zinc-400">
							For more commands, run: <code className="bg-gray-100 dark:bg-zinc-800 px-2 py-1 rounded">opm --help</code>
						</p>
					</div>
				</div>
			</Fragment>
		);
	} else {
		return (
			<Fragment>
				<ToastContainer toasts={toasts} onClose={closeToast} />
				<Header name={`Viewing ${filteredItems.length} of ${items.count()} items`} description="View and manage all the processes on your daemons.">
					<div className="flex gap-2 flex-wrap">
						{showBulkActions && (
							<>
								<span className="inline-flex items-center px-3 py-2 text-sm font-semibold text-gray-600 dark:text-zinc-300 bg-gray-100 dark:bg-zinc-800 rounded-lg border border-gray-300 dark:border-zinc-700">
									{selectedIds.size} selected
								</span>
								<button
									type="button"
									onClick={() => bulkAction('restart')}
									className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-green-700 hover:border-green-600 bg-green-600 text-white hover:bg-green-700 px-3 py-2 text-sm font-semibold rounded-lg">
									Restart
								</button>
								<button
									type="button"
									onClick={() => bulkAction('stop')}
									className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-amber-700 hover:border-amber-600 bg-amber-600 text-white hover:bg-amber-700 px-3 py-2 text-sm font-semibold rounded-lg">
									Stop
								</button>
								<button
									type="button"
									onClick={() => bulkAction('delete')}
									className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-red-700 hover:border-red-600 bg-red-600 text-white hover:bg-red-700 px-3 py-2 text-sm font-semibold rounded-lg">
									Delete
								</button>
								<button
									type="button"
									onClick={clearSelection}
									className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-gray-300 dark:border-zinc-700 hover:border-zinc-600 bg-gray-100 dark:bg-zinc-800 text-gray-900 dark:text-zinc-50 hover:bg-gray-200 dark:bg-zinc-700 px-3 py-2 text-sm font-semibold rounded-lg">
									Clear
								</button>
							</>
						)}
						{!showBulkActions && (
							<button
								type="button"
								onClick={selectAll}
								className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-gray-300 dark:border-zinc-700 hover:border-zinc-600 bg-gray-100 dark:bg-zinc-800 text-gray-900 dark:text-zinc-50 hover:bg-gray-200 dark:bg-zinc-700 px-3 py-2 text-sm font-semibold rounded-lg">
								Select All
							</button>
						)}
						<button
							type="button"
							onClick={saveAll}
							className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-gray-300 dark:border-zinc-700 hover:border-zinc-600 bg-gray-100 dark:bg-zinc-800 text-gray-900 dark:text-zinc-50 hover:bg-gray-200 dark:bg-zinc-700 px-3 py-2 text-sm font-semibold rounded-lg">
							Save
						</button>
						<button
							type="button"
							onClick={restoreAll}
							className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-gray-300 dark:border-zinc-700 hover:border-zinc-600 bg-gray-100 dark:bg-zinc-800 text-gray-900 dark:text-zinc-50 hover:bg-gray-200 dark:bg-zinc-700 px-3 py-2 text-sm font-semibold rounded-lg">
							Restore
						</button>
						<button
							type="button"
							onClick={fetch}
							className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-900 hover:border-gray-200 dark:border-zinc-800 bg-zinc-950 text-white dark:text-zinc-50 hover:bg-white hover:text-gray-900 dark:hover:bg-zinc-900 px-4 py-2 text-sm font-semibold rounded-lg">
							Refresh
						</button>
					</div>
				</Header>
				
				{/* Search and Filter Section */}
				<div className="px-4 sm:px-6 lg:px-8 pb-4 flex flex-col sm:flex-row gap-3 sm:gap-4 items-stretch sm:items-center" role="search" aria-label="Search and filter processes">
					<div className="flex-1">
						<input
							id="search-processes"
							type="text"
							placeholder="Search by name, server, or agent..."
							value={searchTerm}
							onChange={(e) => setSearchTerm(e.target.value)}
							aria-label="Search processes by name, server, or agent"
							className="w-full px-4 py-2.5 bg-white dark:bg-zinc-900/50 border border-gray-300 dark:border-zinc-700/50 rounded-lg text-gray-700 dark:text-zinc-200 placeholder-zinc-500 focus:outline-none focus:ring-2 focus:ring-blue-500/50 focus:border-blue-500 transition-all"
						/>
					</div>
					<div className="sm:w-auto w-full">
						<select
							id="status-filter"
							value={statusFilter}
							onChange={(e) => setStatusFilter(e.target.value)}
							aria-label="Filter processes by status"
							className="w-full sm:w-auto px-4 py-2.5 bg-white dark:bg-zinc-900/50 border border-gray-300 dark:border-zinc-700/50 rounded-lg text-gray-700 dark:text-zinc-200 focus:outline-none focus:ring-2 focus:ring-blue-500/50 focus:border-blue-500 transition-all">
							<option value="all">All Status</option>
							<option value="online">Online</option>
							<option value="stopped">Stopped</option>
							<option value="crashed">Crashed</option>
						</select>
					</div>
					<div className="sm:w-auto w-full">
						<select
							id="agent-filter"
							value={agentFilter}
							onChange={(e) => setAgentFilter(e.target.value)}
							aria-label="Filter processes by agent"
							className="w-full sm:w-auto px-4 py-2.5 bg-white dark:bg-zinc-900/50 border border-gray-300 dark:border-zinc-700/50 rounded-lg text-gray-700 dark:text-zinc-200 focus:outline-none focus:ring-2 focus:ring-blue-500/50 focus:border-blue-500 transition-all">
							<option value="all">All Agents</option>
							<option value="local">Local</option>
							{agents.map((agent) => (
								<option key={agent.id} value={agent.id}>
									{agent.name}
								</option>
							))}
						</select>
					</div>
				</div>

				<ul role="list" className="px-4 sm:px-6 lg:px-8 pb-8 grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4 sm:gap-6 xl:gap-8 fade-in">
					{filteredItems.map((item) => (
						<li key={item.id + item.name} className="group rounded-xl border border-gray-300 dark:border-zinc-700/50 bg-white dark:bg-zinc-900/10 hover:bg-white dark:bg-zinc-900/40 hover:border-zinc-600 relative transition-all duration-300 card-hover shadow-lg hover:shadow-2xl overflow-hidden">
							{/* Selection checkbox */}
							<div className="absolute top-3 left-3 z-10">
								<input
									id={`select-process-${item.id}`}
									type="checkbox"
									checked={selectedIds.has(item.id)}
									onChange={(e) => {
										e.stopPropagation();
										toggleSelect(item.id);
									}}
									className="h-4 w-4 rounded border-zinc-600 bg-gray-100 dark:bg-zinc-800 text-blue-600 focus:ring-2 focus:ring-blue-500 focus:ring-offset-0 focus:ring-offset-zinc-900 cursor-pointer transition-all"
								/>
							</div>
							<div className="flex items-center gap-x-4 border-b border-gray-200 dark:border-zinc-800/80 bg-white dark:bg-zinc-900/30 px-4 py-3.5 pl-12 rounded-t-xl backdrop-blur-sm">
								<div className="flex-1 min-w-0">
									<InlineRename 
										ref={(el) => {
											if (el) {
												renameRefs.current.set(item.id, el);
											} else {
												renameRefs.current.delete(item.id);
											}
										}}
										base={props.base} 
										server={item.server} 
										process_id={item.id} 
										callback={fetch} 
										old={item.name} 
										onSuccess={success} 
										onError={error} 
									/>
									<div className="flex items-center gap-2 mt-0.5">
										<span className={`inline-flex items-center px-2 py-0.5 rounded text-xs font-medium ${
											item.agent_name
												? 'bg-purple-500/10 text-purple-400 border border-purple-500/20'
												: 'bg-blue-500/10 text-blue-400 border border-blue-500/20'
										}`}>
											{item.agent_name || 'local'}
										</span>
									</div>
								</div>
								<span className="relative flex h-2.5 w-2.5 -mt-3.5">
									<span className={`${badge[item.status]} absolute inline-flex h-full w-full rounded-full opacity-75 ${item.status === 'online' ? 'animate-ping' : ''}`}></span>
									<span className={`${badge[item.status]} relative inline-flex rounded-full h-2.5 w-2.5 shadow-lg`}></span>
								</span>
								<Menu as="div" className="relative">
									<MenuButton className="transition border focus:outline-none focus:ring-2 focus:ring-blue-500/50 focus:ring-offset-0 z-50 shrink-0 border-gray-300 dark:border-zinc-700/50 bg-transparent hover:bg-gray-100 dark:bg-zinc-800 hover:border-zinc-600 p-2 text-sm font-semibold rounded-lg">
										<EllipsisVerticalIcon className="h-5 w-5 text-gray-600 dark:text-zinc-300 hover:text-gray-900 dark:text-zinc-50 transition-colors" aria-hidden="true" />
									</MenuButton>
									<Transition
										as={Fragment}
										enter="transition ease-out duration-100"
										enterFrom="transform opacity-0 scale-95"
										enterTo="transform opacity-100 scale-100"
										leave="transition ease-in duration-75"
										leaveFrom="transform opacity-100 scale-100"
										leaveTo="transform opacity-0 scale-95">
										<MenuItems
											anchor={{ to: 'bottom end', gap: '8px', padding: '16px' }}
											className="z-10 w-48 origin-top-right rounded-lg bg-white dark:bg-zinc-900 border border-gray-200 dark:border-zinc-800 shadow-lg ring-1 ring-black ring-opacity-5 focus:outline-none text-base divide-y divide-zinc-800/50">
												<div className="p-1.5">
													{!isRunning(item.status) && (
														<MenuItem>
															{({ focus }) => (
																<a
																	onClick={() => action(item, 'start')}
																	className={classNames(
																		focus ? 'bg-emerald-700/10 text-emerald-500' : 'text-gray-700 dark:text-zinc-200',
																		'rounded-md block px-2 py-2 w-full text-left cursor-pointer'
																	)}>
																	Start
																</a>
															)}
														</MenuItem>
													)}
													<MenuItem>
														{({ focus }) => (
															<a
																onClick={() => action(item, 'restart')}
																className={classNames(
																	focus ? 'bg-green-700/10 text-green-500' : 'text-gray-700 dark:text-zinc-200',
																	'rounded-md block px-2 py-2 w-full text-left cursor-pointer'
																)}>
																Restart
															</a>
														)}
													</MenuItem>
													<MenuItem>
														{({ focus }) => (
															<a
																onClick={() => action(item, 'reload')}
																className={classNames(
																	focus ? 'bg-blue-700/10 text-blue-500' : 'text-gray-700 dark:text-zinc-200',
																	'rounded-md block px-2 py-2 w-full text-left cursor-pointer'
																)}>
																Reload
															</a>
														)}
													</MenuItem>
													<MenuItem>
														{({ focus }) => (
															<a
																onClick={() => action(item, 'stop')}
																className={classNames(
																	focus ? 'bg-yellow-400/10 text-amber-500' : 'text-gray-700 dark:text-zinc-200',
																	'rounded-md block p-2 w-full text-left cursor-pointer'
																)}>
																Terminate
															</a>
														)}
													</MenuItem>
												</div>
												<div className="p-1.5">
													<MenuItem>
														{({ _ }) => (
															<a
																onClick={() => action(item, 'flush')}
																className="text-gray-700 dark:text-zinc-200 rounded-md block p-2 w-full text-left cursor-pointer hover:bg-gray-100 dark:bg-zinc-800/80 hover:text-gray-900 dark:text-zinc-50">
																Clean Logs
															</a>
														)}
													</MenuItem>
												</div>
												<div className="p-1.5">
													<MenuItem>
														{({ focus, close }) => (
															<button
																onClick={(e) => {
																	e.preventDefault();
																	e.stopPropagation();
																	const ref = renameRefs.current.get(item.id);
																	if (ref) {
																		ref.triggerEdit();
																	}
																	close();
																}}
																className={classNames(
																	focus ? 'bg-gray-100 dark:bg-zinc-800/80 text-gray-900 dark:text-zinc-50' : 'text-gray-700 dark:text-zinc-200',
																	'rounded-md block p-2 w-full text-left cursor-pointer'
																)}>
																Rename
															</button>
														)}
													</MenuItem>
												</div>
												<div className="p-1.5">
													<MenuItem>
														{({ focus }) => (
															<a
																onClick={() => action(item, 'delete')}
																className={classNames(
																	focus ? 'bg-red-700/10 text-red-500' : 'text-red-400',
																	'rounded-md block p-2 w-full text-left cursor-pointer'
																)}>
																Delete
															</a>
														)}
													</MenuItem>
												</div>
											</MenuItems>
										</Transition>
								</Menu>
							</div>
							<a href={
								item.agent_id 
									? `./view/${item.id}?agent_id=${item.agent_id}&agent_name=${encodeURIComponent(item.agent_name || item.agent_id)}`
									: isRemote(item) 
										? `./view/${item.id}?server=${item.server}` 
										: `./view/${item.id}`
							} className="block transition-colors duration-200 hover:bg-white dark:bg-zinc-900/20">
								<ProcessDetails item={item} isClickable={true} />
							</a>
						</li>
					))}
				</ul>
			</Fragment>
		);
	}
};

export default Index;
