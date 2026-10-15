-- Two workspaces never share a directory, also after one is closed: gc deletes a closed workspace's directory by its path.
CREATE UNIQUE INDEX workspaces_path ON workspaces (path);
