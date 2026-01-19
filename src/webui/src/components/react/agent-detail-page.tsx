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
				<div className="text-zinc-400 text-lg mb-4">No agent selected</div>
				<div className="text-zinc-500 text-sm">
					Please select an agent from the{' '}
					<a href={`${props.base}/servers`} className="text-blue-500 hover:text-blue-400">
						agents list
					</a>
				</div>
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
