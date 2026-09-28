/** In-flight library → arrange drag path (HTML5 getData is unreliable before drop). */
let activeLibraryDragPath: string | null = null;

export function beginLibraryDrag(path: string) {
	activeLibraryDragPath = path;
}

export function endLibraryDrag() {
	activeLibraryDragPath = null;
}

export function getActiveLibraryDragPath(): string | null {
	return activeLibraryDragPath;
}
