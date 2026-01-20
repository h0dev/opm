import { useEffect, useState } from 'react';
import AgentDetail from '@/components/react/agent-detail';
import Loader from '@/components/react/loader';

const AgentDetailPage = (props: { base: string }) => {
	const [agentId, setAgentId] = useState<string>('');
	const [isInitialized, setIsInitialized] = useState(false);

	useEffect(() => {
		// Get agent ID from URL hash
		const hash = window.location.hash.substring(1);
		if (import.meta.env.DEV) {
			console.log('Agent detail page - hash:', hash);
		}
		setAgentId(hash || '');
		setIsInitialized(true);

		// Listen for hash changes
		const handleHashChange = () => {
			const newHash = window.location.hash.substring(1);
			if (import.meta.env.DEV) {
				console.log('Agent detail page - hash change:', newHash);
			}
			setAgentId(newHash || '');
		};

		window.addEventListener('hashchange', handleHashChange);
		return () => window.removeEventListener('hashchange', handleHashChange);
	}, []);

	// Show loader while initializing
	if (!isInitialized) {
		return <Loader />;
	}

	// Show error if no agent ID
	if (!agentId) {
		return (
			<div className="px-4 sm:px-6 lg:px-8 text-center py-12">
				<div className="text-zinc-200 text-xl font-semibold mb-4">No Agent Selected</div>
				<div className="text-zinc-300 text-base mb-6">
					Please select an agent from the agents list to view details.
				</div>
				<a 
					href={`${props.base}/servers`} 
					className="inline-flex items-center justify-center px-4 py-2 text-sm font-semibold text-white bg-blue-600 hover:bg-blue-700 rounded-lg transition">
					Go to Agents List
				</a>
			</div>
		);
	}

	return (
		<div className="px-4 sm:px-6 lg:px-8">
			<AgentDetail agentId={agentId} base={props.base} />
		</div>
	);
};

export default AgentDetailPage;
