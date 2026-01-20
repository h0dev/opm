export default () => (
	<div
		style={{
			position: 'fixed',
			top: '50%',
			left: '50%',
			transform: 'translate(-50%, -50%)',
			pointerEvents: 'none',
			zIndex: 50
		}}>
		<div className="space-y-4">
			<div className="h-2 w-80 sm:w-96 bg-zinc-700 overflow-hidden rounded-full shadow-lg">
				<div className="animate-progress w-full h-full bg-gradient-to-r from-blue-500 via-purple-500 to-pink-500 origin-left-right shadow-glow"></div>
			</div>
			<div className="text-center text-zinc-200 text-base font-medium animate-pulse">Loading...</div>
		</div>
	</div>
);
