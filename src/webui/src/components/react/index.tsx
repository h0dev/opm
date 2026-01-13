import { api } from '@/api';
import Rename from '@/components/react/rename';
import Loader from '@/components/react/loader';
import Header from '@/components/react/header';
import { useArray, classNames } from '@/helpers';
import { useEffect, useState, Fragment } from 'react';
import { EllipsisVerticalIcon } from '@heroicons/react/20/solid';
import { Menu, MenuItem, MenuItems, MenuButton, Transition } from '@headlessui/react';

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
};

const Index = (props: { base: string }) => {
	const items = useArray<ProcessItem>([]);
	const [searchTerm, setSearchTerm] = useState('');
	const [statusFilter, setStatusFilter] = useState('all');
	const [selectedIds, setSelectedIds] = useState<Set<number>>(new Set());
	const [showBulkActions, setShowBulkActions] = useState(false);

	const badge = {
		online: 'bg-emerald-400',
		stopped: 'bg-red-500',
		crashed: 'bg-amber-400'
	};

	async function fetch() {
		items.clear();
		setSelectedIds(new Set()); // Clear selections on refresh

		const res = await api.get(props.base + '/list').json();
		res.map((s) => items.push({ ...s, server: 'local' }));

		try {
			const servers = await api.get(props.base + '/daemon/servers').json();
			await servers.forEach(async (name) => {
				const remote = await api.get(props.base + `/remote/${name}/list`).json();
				remote.map((s) => items.push({ ...s, server: name }));
			});
		} catch {}
	}

	const isRemote = (item: ProcessItem): boolean => item.server !== 'local';
	const isRunning = (status: string): boolean => !['stopped', 'crashed'].includes(status);
	const action = (item: ProcessItem, name: string) => {
		const endpoint = item.server === 'local' 
			? `${props.base}/process/${item.id}/action`
			: `${props.base}/remote/${item.server}/action/${item.id}`;
		return api.post(endpoint, { json: { method: name } }).then(() => fetch());
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
			await fetch();
			alert(`${method} action completed on ${selectedIds.size} processes`);
		} catch (error) {
			alert('Failed to perform bulk action: ' + (error as Error).message);
		}
	};
	
	// Save all processes
	const saveAll = async () => {
		try {
			await api.post(`${props.base}/daemon/save`, {});
			alert('All processes saved to dumpfile');
		} catch (error) {
			alert('Failed to save processes: ' + (error as Error).message);
		}
	};
	
	// Restore all processes
	const restoreAll = async () => {
		try {
			await api.post(`${props.base}/daemon/restore`, {});
			fetch();
			alert('All processes restored from dumpfile');
		} catch (error) {
			alert('Failed to restore processes: ' + (error as Error).message);
		}
	};

	// Filter items based on search term and status filter
	const filteredItems = items.value.filter((item) => {
		const matchesSearch = searchTerm === '' || 
			item.name.toLowerCase().includes(searchTerm.toLowerCase()) ||
			item.server.toLowerCase().includes(searchTerm.toLowerCase());
		
		const matchesStatus = statusFilter === 'all' || item.status === statusFilter;
		
		return matchesSearch && matchesStatus;
	});

	useEffect(() => {
		fetch();
	}, []);

	if (items.isEmpty()) {
		return <Loader />;
	} else {
		return (
			<Fragment>
				<Header name={`Viewing ${filteredItems.length} of ${items.count()} items`} description="View and manage all the processes on your daemons.">
					<div className="flex gap-2 flex-wrap">
						{showBulkActions && (
							<>
								<span className="inline-flex items-center px-3 py-2 text-sm font-semibold text-zinc-300 bg-zinc-800 rounded-lg border border-zinc-700">
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
									className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-700 hover:border-zinc-600 bg-zinc-800 text-zinc-50 hover:bg-zinc-700 px-3 py-2 text-sm font-semibold rounded-lg">
									Clear
								</button>
							</>
						)}
						{!showBulkActions && (
							<>
								<button
									type="button"
									onClick={selectAll}
									className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-700 hover:border-zinc-600 bg-zinc-800 text-zinc-50 hover:bg-zinc-700 px-3 py-2 text-sm font-semibold rounded-lg">
									Select All
								</button>
								<button
									type="button"
									onClick={saveAll}
									className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-700 hover:border-zinc-600 bg-zinc-800 text-zinc-50 hover:bg-zinc-700 px-3 py-2 text-sm font-semibold rounded-lg">
									Save All
								</button>
								<button
									type="button"
									onClick={restoreAll}
									className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-700 hover:border-zinc-600 bg-zinc-800 text-zinc-50 hover:bg-zinc-700 px-3 py-2 text-sm font-semibold rounded-lg">
									Restore All
								</button>
							</>
						)}
						<button
							type="button"
							onClick={fetch}
							className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-900 hover:border-zinc-800 bg-zinc-950 text-zinc-50 hover:bg-zinc-900 px-4 py-2 text-sm font-semibold rounded-lg">
							Refresh
						</button>
					</div>
				</Header>
				
				{/* Search and Filter Section */}
				<div className="px-4 sm:px-6 lg:px-8 pb-4 flex flex-col sm:flex-row gap-3 sm:gap-4 items-stretch sm:items-center" role="search" aria-label="Search and filter processes">
					<div className="flex-1">
						<input
							type="text"
							placeholder="Search by name or server..."
							value={searchTerm}
							onChange={(e) => setSearchTerm(e.target.value)}
							aria-label="Search processes by name or server"
							className="w-full px-4 py-2.5 bg-zinc-900/50 border border-zinc-700/50 rounded-lg text-zinc-200 placeholder-zinc-500 focus:outline-none focus:ring-2 focus:ring-blue-500/50 focus:border-blue-500 transition-all"
						/>
					</div>
					<div className="sm:w-auto w-full">
						<select
							value={statusFilter}
							onChange={(e) => setStatusFilter(e.target.value)}
							aria-label="Filter processes by status"
							className="w-full sm:w-auto px-4 py-2.5 bg-zinc-900/50 border border-zinc-700/50 rounded-lg text-zinc-200 focus:outline-none focus:ring-2 focus:ring-blue-500/50 focus:border-blue-500 transition-all">
							<option value="all">All Status</option>
							<option value="online">Online</option>
							<option value="stopped">Stopped</option>
							<option value="crashed">Crashed</option>
						</select>
					</div>
				</div>

				<ul role="list" className="px-4 sm:px-6 lg:px-8 pb-8 grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-4 sm:gap-6 xl:gap-8 fade-in">
					{filteredItems.map((item) => (
						<li key={item.id + item.name} className="rounded-xl border border-zinc-700/50 bg-zinc-900/10 hover:bg-zinc-900/40 hover:border-zinc-600 relative transition-all duration-300 card-hover shadow-lg hover:shadow-2xl overflow-hidden">
							{/* Selection checkbox */}
							<div className="absolute top-3 left-3 z-10">
								<input
									type="checkbox"
									checked={selectedIds.has(item.id)}
									onChange={(e) => {
										e.stopPropagation();
										toggleSelect(item.id);
									}}
									className="h-4 w-4 rounded border-zinc-600 bg-zinc-800 text-blue-600 focus:ring-2 focus:ring-blue-500 focus:ring-offset-0 focus:ring-offset-zinc-900 cursor-pointer transition-all"
								/>
							</div>
							<div className="flex items-center gap-x-4 border-b border-zinc-800/80 bg-zinc-900/30 px-4 py-3.5 pl-12 rounded-t-xl backdrop-blur-sm">
								<span className="text-md font-bold text-zinc-200 truncate flex-1">
									{item.name}
									<div className="text-xs font-medium text-zinc-400 mt-0.5">{item.server != 'local' ? item.server : 'Internal'}</div>
								</span>
								<span className="relative flex h-2.5 w-2.5 -mt-3.5">
									<span className={`${badge[item.status]} absolute inline-flex h-full w-full rounded-full opacity-75 ${item.status === 'online' ? 'animate-ping' : ''}`}></span>
									<span className={`${badge[item.status]} relative inline-flex rounded-full h-2.5 w-2.5 shadow-lg`}></span>
								</span>
								<Menu as="div" className="relative">
									<MenuButton className="transition border focus:outline-none focus:ring-2 focus:ring-blue-500/50 focus:ring-offset-0 z-50 shrink-0 border-zinc-700/50 bg-transparent hover:bg-zinc-800 hover:border-zinc-600 p-2 text-sm font-semibold rounded-lg">
										<EllipsisVerticalIcon className="h-5 w-5 text-zinc-300 hover:text-zinc-50 transition-colors" aria-hidden="true" />
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
											className="z-10 w-48 origin-top-right rounded-lg bg-zinc-900 border border-zinc-800 shadow-lg ring-1 ring-black ring-opacity-5 focus:outline-none text-base divide-y divide-zinc-800/50">
											<div className="p-1.5">
												{!isRunning(item.status) && (
													<MenuItem>
														{({ focus }) => (
															<a
																onClick={() => action(item, 'start')}
																className={classNames(
																	focus ? 'bg-emerald-700/10 text-emerald-500' : 'text-zinc-200',
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
																focus ? 'bg-green-700/10 text-green-500' : 'text-zinc-200',
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
																focus ? 'bg-blue-700/10 text-blue-500' : 'text-zinc-200',
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
																focus ? 'bg-yellow-400/10 text-amber-500' : 'text-zinc-200',
																'rounded-md block p-2 w-full text-left cursor-pointer'
															)}>
															Terminate
														</a>
													)}
												</MenuItem>
											</div>
											<div className="p-1.5">
												<MenuItem>
													{({ _ }) => <Rename base={props.base} server={item.server} process_id={item.id} callback={fetch} old={item.name} />}
												</MenuItem>
												<MenuItem>
													{({ _ }) => (
														<a
															onClick={() => action(item, 'flush')}
															className="text-zinc-200 rounded-md block p-2 w-full text-left cursor-pointer hover:bg-zinc-800/80 hover:text-zinc-50">
															Clean Logs
														</a>
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
							<a href={isRemote(item) ? `./view/${item.id}?server=${item.server}` : `./view/${item.id}`} className="block transition-colors duration-200 hover:bg-zinc-900/20">
								<dl className="-my-3 divide-y divide-zinc-800/30 px-6 py-4 text-sm leading-6">
									<div className="flex justify-between gap-x-2 py-2 transition-colors hover:text-zinc-300">
										<dt className="text-zinc-600 font-medium">cpu usage</dt>
										<dd className="text-zinc-400 font-mono">{isRunning(item.status) ? item.cpu : 'offline'}</dd>
									</div>
									<div className="flex justify-between gap-x-2 py-2 transition-colors hover:text-zinc-300">
										<dt className="text-zinc-600 font-medium">memory</dt>
										<dd className="text-zinc-400 font-mono">{isRunning(item.status) ? item.mem : 'offline'}</dd>
									</div>
									<div className="flex justify-between gap-x-2 py-2 transition-colors hover:text-zinc-300">
										<dt className="text-zinc-600 font-medium">pid</dt>
										<dd className="text-zinc-400 font-mono">{isRunning(item.status) ? item.pid : 'none'}</dd>
									</div>
									<div className="flex justify-between gap-x-2 py-2 transition-colors hover:text-zinc-300">
										<dt className="text-zinc-600 font-medium">uptime</dt>
										<dd className="text-zinc-400 font-mono">{isRunning(item.status) ? item.uptime : 'none'}</dd>
									</div>
									<div className="flex justify-between gap-x-2 py-2 transition-colors hover:text-zinc-300">
										<dt className="text-zinc-600 font-medium">restarts</dt>
										<dd className="text-zinc-400 font-mono">{item.restarts == 0 ? 'none' : item.restarts}</dd>
									</div>
								</dl>
							</a>
						</li>
					))}
				</ul>
			</Fragment>
		);
	}
};

export default Index;
