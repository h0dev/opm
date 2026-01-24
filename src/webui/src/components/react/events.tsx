import { SSE, headers } from '@/api';
import { useEffect, Fragment, useState } from 'react';
import Loader from '@/components/react/loader';
import Header from '@/components/react/header';
import ToastContainer from '@/components/react/toast';
import { useToast } from '@/components/react/useToast';

type EventType = 'agentconnect' | 'agentdisconnect' | 'processstart' | 'processstop' | 'processcrash' | 'processrestart' | 'processdelete';

interface Event {
	id: string;
	timestamp: string;
	event_type: EventType;
	agent_id: string;
	agent_name: string;
	process_id?: string;
	process_name?: string;
	message: string;
}

const EventsPage = (props: { base: string }) => {
	const { toasts, closeToast } = useToast();
	const [events, setEvents] = useState<Event[]>([]);
	const [loading, setLoading] = useState(true);
	const [retryTrigger, setRetryTrigger] = useState(0);

	const eventColors: Record<EventType, string> = {
		agentconnect: 'bg-emerald-500/10 text-emerald-400 border-emerald-500/20',
		agentdisconnect: 'bg-red-500/10 text-red-400 border-red-500/20',
		processstart: 'bg-blue-500/10 text-blue-400 border-blue-500/20',
		processstop: 'bg-yellow-500/10 text-yellow-400 border-yellow-500/20',
		processcrash: 'bg-red-500/10 text-red-400 border-red-500/20',
		processrestart: 'bg-purple-500/10 text-purple-400 border-purple-500/20',
		processdelete: 'bg-orange-500/10 text-orange-400 border-orange-500/20',
	};

	const eventIcons: Record<EventType, string> = {
		agentconnect: '🟢',
		agentdisconnect: '🔴',
		processstart: '▶️',
		processstop: '⏹️',
		processcrash: '💥',
		processrestart: '🔄',
		processdelete: '🗑️',
	};

	const formatEventType = (type: EventType): string => {
		const typeMap: Record<EventType, string> = {
			agentconnect: 'Agent Connect',
			agentdisconnect: 'Agent Disconnect',
			processstart: 'Process Start',
			processstop: 'Process Stop',
			processcrash: 'Process Crash',
			processrestart: 'Process Restart',
			processdelete: 'Process Delete',
		};
		return typeMap[type] || type;
	};

	const formatTimestamp = (timestamp: string): string => {
		const date = new Date(timestamp);
		const now = new Date();
		const diffMs = now.getTime() - date.getTime();
		const diffMins = Math.floor(diffMs / 60000);
		const diffHours = Math.floor(diffMs / 3600000);
		const diffDays = Math.floor(diffMs / 86400000);

		if (diffMins < 1) return 'Just now';
		if (diffMins < 60) return `${diffMins}m ago`;
		if (diffHours < 24) return `${diffHours}h ago`;
		if (diffDays < 7) return `${diffDays}d ago`;
		
		return date.toLocaleString();
	};

	const handleRefresh = () => {
		setLoading(true);
		setRetryTrigger(prev => prev + 1);
	};

	useEffect(() => {
		setLoading(true);
		
		const source = new SSE(`${props.base}/live/events`, { headers });
		
		source.onmessage = (event) => {
			try {
				const data = JSON.parse(event.data);
				if (Array.isArray(data)) {
					setEvents(data);
				}
				setLoading(false);
			} catch (err) {
				console.error('Failed to parse SSE data:', err);
				setLoading(false);
			}
		};
		
		source.onerror = (err) => {
			console.error('SSE connection error:', err);
			setLoading(false);
		};
		
		source.stream();
		
		return () => {
			source.close();
		};
	}, [retryTrigger]);

	if (loading) {
		return <Loader />;
	}

	return (
		<Fragment>
			<ToastContainer toasts={toasts} onClose={closeToast} />
			<Header name="Events" description="System events for process and agent lifecycle changes.">
				<div className="flex gap-2">
					<button
						type="button"
						onClick={handleRefresh}
						className="transition inline-flex items-center justify-center space-x-1.5 border focus:outline-none focus:ring-0 focus:ring-offset-0 focus:z-10 shrink-0 border-zinc-900 hover:border-gray-200 dark:border-zinc-800 bg-zinc-950 text-gray-900 dark:text-zinc-50 hover:bg-white dark:bg-zinc-900 px-4 py-2 text-sm font-semibold rounded-lg">
						Refresh
					</button>
				</div>
			</Header>

			<div className="space-y-4 px-4 sm:px-6 lg:px-8">
				{events.length === 0 ? (
					<div className="text-center py-12">
						<div className="text-gray-500 dark:text-zinc-400 text-lg mb-2">No events yet</div>
						<div className="text-gray-900 dark:text-gray-400 dark:text-zinc-500 text-sm">
							Events will appear here when processes start, stop, crash, or agents connect/disconnect.
						</div>
					</div>
				) : (
					<div className="space-y-3">
						{(() => {
							// Group similar events within 5 minute windows
							const groupedEvents: Array<{ event: Event; count: number; latestTimestamp: string }> = [];
							const TIME_WINDOW_MS = 5 * 60 * 1000; // 5 minutes
							
							events.forEach((event) => {
								// Find a similar event in the last 5 minutes
								const similarGroup = groupedEvents.find((group) => {
									const timeDiff = new Date(event.timestamp).getTime() - new Date(group.latestTimestamp).getTime();
									return (
										group.event.event_type === event.event_type &&
										group.event.agent_id === event.agent_id &&
										group.event.process_id === event.process_id &&
										Math.abs(timeDiff) < TIME_WINDOW_MS
									);
								});
								
								if (similarGroup) {
									// Increment count and update to latest timestamp
									similarGroup.count++;
									if (new Date(event.timestamp) > new Date(similarGroup.latestTimestamp)) {
										similarGroup.latestTimestamp = event.timestamp;
										similarGroup.event = event; // Update to show latest message
									}
								} else {
									// Create new group
									groupedEvents.push({
										event,
										count: 1,
										latestTimestamp: event.timestamp
									});
								}
							});
							
							return groupedEvents.map((group) => (
								<div
									key={group.event.id}
									className={`border rounded-lg p-4 ${eventColors[group.event.event_type]}`}>
									<div className="flex items-start justify-between gap-4">
										<div className="flex items-start gap-3 flex-1">
											<div className="text-2xl flex-shrink-0 mt-1">
												{eventIcons[group.event.event_type]}
											</div>
											<div className="flex-1 min-w-0">
												<div className="flex items-center gap-2 mb-1">
													<span className="font-semibold text-sm">
														{formatEventType(group.event.event_type)}
													</span>
													{group.count > 1 && (
														<span className="bg-gray-100 dark:bg-zinc-800 text-gray-700 dark:text-zinc-300 px-2 py-0.5 rounded-full text-xs font-medium">
															×{group.count}
														</span>
													)}
													<span className="text-xs opacity-70">
														{formatTimestamp(group.latestTimestamp)}
													</span>
												</div>
												<div className="text-sm mb-2">{group.event.message}</div>
												<div className="flex flex-wrap gap-2 text-xs opacity-80">
													<span className="bg-gray-100 dark:bg-zinc-800/50 px-2 py-1 rounded">
														Agent: {group.event.agent_name}
													</span>
													{group.event.process_name && (
														<span className="bg-gray-100 dark:bg-zinc-800/50 px-2 py-1 rounded">
															Process: {group.event.process_name}
														</span>
													)}
												</div>
											</div>
										</div>
									</div>
								</div>
							));
						})()}
					</div>
				)}
			</div>
		</Fragment>
	);
};

export default EventsPage;
