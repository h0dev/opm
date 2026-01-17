export default (props: { name: string; description: string; children }) => (
	<div class="flex flex-col sm:flex-row items-start sm:items-center justify-between p-4 sm:p-6 lg:p-8 border-b border-zinc-800/50 backdrop-blur-sm bg-zinc-900/20">
		<div class="flex-shrink-0 text-left">
			<h1 class="text-lg sm:text-xl font-bold leading-6 text-white gradient-text">{props.name}</h1>
			<p class="mt-2 text-sm text-zinc-400">{props.description}</p>
		</div>
		<div class="mt-4 sm:ml-16 sm:mt-0 flex-shrink-0 flex flex-wrap gap-2 sm:justify-end">{props.children}</div>
	</div>
);
