// The plugin store and registries class of the binding (8A Tasks 8 and 9a). A factory over the
// loader's `native` binding and error adapter, so `index.js` stays small and this file has no
// import cycle with it.

/** @param {{ native: any, call: (p: Promise<any>) => Promise<any>, toFinError: (e: unknown) => unknown }} deps */
export function defineRegistry({ native, call, toFinError }) {
  return class Registry {
    #native;

    constructor(nativeRegistry) {
      if (!(nativeRegistry instanceof native.Registry)) throw new TypeError("use Registry.open");
      this.#native = nativeRegistry;
    }

    static open(serverData) {
      try {
        return new Registry(native.Registry.open(serverData));
      } catch (e) {
        throw toFinError(e);
      }
    }

    plan(reqJson) {
      return call(this.#native.plan(reqJson));
    }

    commit(planId, digest, acceptJson) {
      return call(this.#native.commit(planId, digest, acceptJson ?? null));
    }

    removePlugin(id, version, purgeData) {
      return call(this.#native.removePlugin(id, version, purgeData === true));
    }

    sweep() {
      return call(this.#native.sweep());
    }

    registryList() {
      return call(this.#native.registryList());
    }

    registryStatus(id) {
      return call(this.#native.registryStatus(id));
    }

    registryAdd(cfgJson, rootJson) {
      return call(this.#native.registryAdd(cfgJson, rootJson));
    }

    registryRemove(id) {
      return call(this.#native.registryRemove(id));
    }

    registryForget(rootSha256) {
      return call(this.#native.registryForget(rootSha256));
    }

    registryRefresh(id, byHuman) {
      return call(this.#native.registryRefresh(id, byHuman === true));
    }

    search(queryJson) {
      return call(this.#native.search(queryJson));
    }

    info(id) {
      return call(this.#native.info(id));
    }
  };
}
