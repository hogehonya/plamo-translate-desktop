// Firefox does not implement FileSystemHandle.move. Stream the completed download
// into its final name; preserve the partial file if the copy fails.
export async function commitModelFile(part, dir, name) {
  if (typeof part.move === "function") {
    await part.move(dir, name);
    return;
  }
  const file = await part.getFile();
  const target = await dir.getFileHandle(name, { create: true });
  await file.stream().pipeTo(await target.createWritable());
  await dir.removeEntry(part.name);
}
