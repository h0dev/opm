export default (props: { name: string; description: string; children }) => (
	<div class="sm:flex sm:items-center sm:justify-start p-4 sm:p-6 lg:p-8 border-b border-zinc-800/50 backdrop-blur-sm bg-zinc-900/20">
		<div class="sm:flex-auto text-left">
			<h1 class="text-lg sm:text-xl font-bold leading-6 text-white gradient-text">{props.name}</h1>
			<p class="mt-2 text-sm text-zinc-400">{props.description}</p>
		</div>
		<div class="mt-4 sm:ml-16 sm:mt-0 sm:flex-none flex flex-wrap gap-2">{props.children}</div>
	</div>
);
