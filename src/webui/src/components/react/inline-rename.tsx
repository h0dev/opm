import { api } from '@/api';
import { useEffect, useState, useRef } from 'react';
import { CheckIcon, XMarkIcon, PencilIcon } from '@heroicons/react/20/solid';

interface InlineRenameProps {
	base: string;
	server: string;
	process_id: number;
	callback: () => void;
	old: string;
	onSuccess?: (msg: string) => void;
	onError?: (msg: string) => void;
}

const InlineRename = (props: InlineRenameProps) => {
	const [isEditing, setIsEditing] = useState(false);
	const [formData, setFormData] = useState('');
	const inputRef = useRef<HTMLInputElement>(null);

	const handleChange = (event: React.ChangeEvent<HTMLInputElement>) => setFormData(event.target.value);

	const handleSave = async () => {
		const url =
			props.server !== 'local'
				? `${props.base}/remote/${props.server}/rename/${props.process_id}`
				: `${props.base}/process/${props.process_id}/rename`;

		try {
			await api.post(url, { body: formData });
			setIsEditing(false);
			props.callback();
			props.onSuccess?.('Process renamed successfully');
		} catch (err) {
			props.onError?.(`Failed to rename process: ${err instanceof Error ? err.message : 'Unknown error'}`);
		}
	};

	const handleCancel = () => {
		setFormData(props.old);
		setIsEditing(false);
	};

	const handleKeyDown = (e: React.KeyboardEvent) => {
		if (e.key === 'Enter') {
			e.preventDefault();
			handleSave();
		} else if (e.key === 'Escape') {
			handleCancel();
		}
	};

	useEffect(() => {
		setFormData(props.old);
	}, [props.old]);

	useEffect(() => {
		if (isEditing && inputRef.current) {
			inputRef.current.focus();
			inputRef.current.select();
		}
	}, [isEditing]);

	if (!isEditing) {
		return (
			<div className="flex items-center gap-2">
				<span className="text-md font-bold text-zinc-200 truncate">
					{props.old}
				</span>
				<button
					onClick={(e) => {
						e.preventDefault();
						e.stopPropagation();
						setIsEditing(true);
					}}
					className="opacity-0 group-hover:opacity-100 transition-opacity p-1 hover:bg-zinc-800 rounded text-zinc-400 hover:text-zinc-200"
					title="Rename">
					<PencilIcon className="h-4 w-4" />
				</button>
			</div>
		);
	}

	return (
		<div className="flex items-center gap-2" onClick={(e) => e.stopPropagation()}>
			<input
				ref={inputRef}
				type="text"
				value={formData}
				onChange={handleChange}
				onKeyDown={handleKeyDown}
				onClick={(e) => e.stopPropagation()}
				className="flex-1 px-2 py-1 bg-zinc-800 border border-zinc-600 rounded text-zinc-100 text-sm focus:outline-none focus:ring-2 focus:ring-blue-500 focus:border-blue-500"
			/>
			<button
				onClick={(e) => {
					e.stopPropagation();
					handleSave();
				}}
				className="p-1 bg-green-600 hover:bg-green-700 rounded text-white transition-colors"
				title="Save">
				<CheckIcon className="h-4 w-4" />
			</button>
			<button
				onClick={(e) => {
					e.stopPropagation();
					handleCancel();
				}}
				className="p-1 bg-red-600 hover:bg-red-700 rounded text-white transition-colors"
				title="Cancel">
				<XMarkIcon className="h-4 w-4" />
			</button>
		</div>
	);
};

export default InlineRename;
