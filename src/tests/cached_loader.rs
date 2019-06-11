use std::{collections::BTreeMap, sync::Arc, thread};

use futures::{future, TryFutureExt as _};
use tokio::runtime::current_thread;

use super::*;

#[test]
fn assert_kinds() {
    fn _assert_send<T: Send>() {}
    fn _assert_sync<T: Sync>() {}
    fn _assert_clone<T: Clone>() {}
    _assert_send::<
        cached::Loader<u32, u32, u32, Batcher, BTreeMap<u32, Result<u32, LoadError<u32>>>>,
    >();
    _assert_sync::<
        cached::Loader<u32, u32, u32, Batcher, BTreeMap<u32, Result<u32, LoadError<u32>>>>,
    >();
    _assert_clone::<
        cached::Loader<u32, u32, u32, Batcher, BTreeMap<u32, Result<u32, LoadError<u32>>>>,
    >();
}

#[test]
fn smoke() {
    let mut rt = current_thread::Runtime::new().unwrap();

    let loader = Loader::new(Batcher::new(2)).cached();

    let v = future::try_join3(loader.load(1), loader.load(2), loader.load(3));
    let one_by_one = rt.block_on(v.boxed_local().compat()).unwrap();
    assert_eq!((10, 20, 30), one_by_one);

    let v = loader.load_many(vec![10, 20, 30]);
    let many = rt.block_on(v.boxed_local().compat()).unwrap();
    assert_eq!(vec![100, 200, 300], many);

    let loader_ref = &loader;
    {
        let v = future::try_join(loader_ref.load(1), loader_ref.load(2));
        let one_by_one_ref = rt.block_on(v.boxed_local().compat()).unwrap();
        assert_eq!((10, 20), one_by_one_ref);
    }
    {
        let v1 = loader_ref.load(3).and_then(|v| loader_ref.load(v));
        let v2 = loader_ref.load(4).and_then(|v| loader_ref.load(v));
        let v = future::try_join(v1, v2);
        let one_by_one_map_ref = rt.block_on(v.boxed_local().compat()).unwrap();
        assert_eq!((300, 400), one_by_one_map_ref);
    }
}

#[test]
fn drop_loader() {
    let mut rt = current_thread::Runtime::new().unwrap();

    let all = {
        let loader = Loader::new(Batcher::new(10)).cached();
        let v1 = loader.load(1);
        let v2 = loader.load(2);
        drop(loader);
        future::try_join(v1, v2)
    };

    assert_eq!((10, 20), rt.block_on(all.boxed_local().compat()).unwrap());
}

#[test]
fn dispatch_partial_batch() {
    let mut rt = current_thread::Runtime::new().unwrap();

    let loader = Loader::new(Batcher::new(10)).cached();
    let all = future::try_join(loader.load(1), loader.load(2));

    assert_eq!((10, 20), rt.block_on(all.boxed_local().compat()).unwrap());
}

#[test]
fn nested_load() {
    let mut rt = current_thread::Runtime::new().unwrap();

    let loader = Loader::new(Batcher::new(2)).cached();
    let v1 = loader.load(3).and_then(|v| loader.load(v));
    let v2 = loader.load(4).and_then(|v| loader.load(v));
    let all = future::try_join(v1, v2);

    assert_eq!((300, 400), rt.block_on(all.boxed_local().compat()).unwrap());
}

#[test]
fn nested_load_many() {
    let mut rt = current_thread::Runtime::new().unwrap();

    let loader = Loader::new(Batcher::new(2)).cached();
    let v1 = loader
        .load(3)
        .and_then(|v| loader.load_many(vec![v, v + 1, v + 2]));
    let v2 = loader
        .load(4)
        .and_then(|v| loader.load_many(vec![v, v + 1, v + 2]));
    let all = future::try_join(v1, v2);

    let expected = (vec![300, 310, 320], vec![400, 410, 420]);
    assert_eq!(expected, rt.block_on(all.boxed_local().compat()).unwrap());
}

#[test]
fn test_batch_fn_error() {
    let mut rt = current_thread::Runtime::new().unwrap();

    let loader = Loader::<i32, i32, MyError, BadBatcher>::new(BadBatcher).cached();
    let v1 = rt.block_on(loader.load(1).boxed_local().compat());

    assert_eq!(LoadError::BatchFn(MyError::Unknown), v1.err().unwrap());
}

#[test]
fn test_result_val() {
    let mut rt = current_thread::Runtime::new().unwrap();

    let loader =
        Loader::<i32, Result<i32, ValueError>, MyError, BadBatcher>::new(BadBatcher).cached();
    let v1 = rt.block_on(loader.load_many(vec![1, 2]).boxed_local().compat());

    assert_eq!(vec![Err(ValueError::NotEven), Ok(20)], v1.unwrap());
}

#[test]
fn test_batch_call_seq() {
    let mut rt = current_thread::Runtime::new().unwrap();

    // batch size = 2, value will be (batch_fn call seq, v * 10)
    let loader = Loader::<i32, (usize, i32), (), _>::new(Batcher::new(2)).cached();
    let v1 = loader.load(1);
    let v2 = loader.load(2);
    let v3 = loader.load(3);
    let v4 = loader.load(4);
    let v5 = loader.load(1);
    let v6 = loader.load(2);

    //v1 and v2 should be in first batch
    let b1 = future::try_join(v1, v2);
    assert_eq!(
        ((1, 10), (1, 20)),
        rt.block_on(b1.boxed_local().compat()).unwrap()
    );
    //v3 and v4 should be in second batch
    let b2 = future::try_join(v3, v4);
    assert_eq!(
        ((2, 30), (2, 40)),
        rt.block_on(b2.boxed_local().compat()).unwrap()
    );
    //v5 and v6 should be using cache of first batch
    let c1 = future::try_join(v5, v6);
    assert_eq!(
        ((1, 10), (1, 20)),
        rt.block_on(c1.boxed_local().compat()).unwrap()
    );
}

#[test]
fn pass_to_thread() {
    let loader = Loader::new(Batcher::new(4)).cached();

    let l = loader.clone();
    let h1 = thread::spawn(move || {
        let mut rt = current_thread::Runtime::new().unwrap();
        let all = future::try_join(l.load(1), l.load(2));
        assert_eq!((10, 20), rt.block_on(all.boxed_local().compat()).unwrap());
    });

    let l2 = loader.clone();
    let h2 = thread::spawn(move || {
        let mut rt = current_thread::Runtime::new().unwrap();
        let all = future::try_join(l2.load(1), l2.load(2));
        assert_eq!((10, 20), rt.block_on(all.boxed_local().compat()).unwrap());
    });

    let _ = h1.join();
    let _ = h2.join();
}

#[test]
fn test_run_by_tokio_runtime() {
    let mut rt = tokio::runtime::Runtime::new().unwrap();

    let loader = Arc::new(Loader::new(Batcher::new(10)).cached());
    let loader2 = loader.clone();
    let v1 = loader
        .clone()
        .load(3)
        .and_then(move |v| loader.load_many(vec![v, v + 1, v + 2]));
    let v2 = loader2
        .clone()
        .load(4)
        .and_then(move |v| loader2.load_many(vec![v, v + 1, v + 2]));
    let all = future::try_join(v1, v2);

    let output = rt.block_on(all.boxed().compat()).unwrap();
    let expected = (vec![300, 310, 320], vec![400, 410, 420]);
    assert_eq!(expected, output);
}

#[test]
fn test_clear() {
    let mut rt = current_thread::Runtime::new().unwrap();

    // batch size = 2, value will be (batch_fn call seq, v * 10)
    let loader = Loader::<i32, (usize, i32), (), _>::new(Batcher::new(2)).cached();
    let v1 = loader.load(1);
    let v2 = loader.load(1);
    let all = future::try_join(v1, v2);
    assert_eq!(
        ((1, 10), (1, 10)),
        rt.block_on(all.boxed_local().compat()).unwrap(),
    );

    loader.remove(&1);
    let v3 = loader.load(1);
    assert_eq!((2, 10), rt.block_on(v3.boxed_local().compat()).unwrap());
}

#[test]
fn test_clear_all() {
    let mut rt = current_thread::Runtime::new().unwrap();

    // batch size = 2, value will be (batch_fn call seq, v * 10)
    let loader = Loader::<i32, (usize, i32), (), _>::new(Batcher::new(2)).cached();
    let v1 = loader.load(1);
    let v2 = loader.load(2);
    let all = future::try_join(v1, v2);
    assert_eq!(
        ((1, 10), (1, 20)),
        rt.block_on(all.boxed_local().compat()).unwrap(),
    );

    loader.clear();
    let v3 = loader.load(1);
    let v4 = loader.load(2);
    let all = future::try_join(v3, v4);
    assert_eq!(
        ((2, 10), (2, 20)),
        rt.block_on(all.boxed_local().compat()).unwrap(),
    );
}

#[test]
fn test_prime() {
    let mut rt = current_thread::Runtime::new().unwrap();

    // batch size = 1, value will be (batch_fn call seq, v * 10)
    let loader = Loader::<i32, (usize, i32), (), _>::new(Batcher::new(1)).cached();
    loader.prime(1, (0, 101));
    let v1 = loader.load(1);
    let v2 = loader.load(2);
    assert_eq!((0, 101), rt.block_on(v1.boxed_local().compat()).unwrap());
    assert_eq!((1, 20), rt.block_on(v2.boxed_local().compat()).unwrap());

    loader.prime(2, (0, 201)); // should have no effect as key 2 are loaded already
    let v3 = loader.load(2);
    assert_eq!((1, 20), rt.block_on(v3.boxed_local().compat()).unwrap());
}

#[test]
fn test_custom_cache() {
    let mut rt = current_thread::Runtime::new().unwrap();

    // batch size = 2, value will be (batch_fn call seq, v * 10)
    let loader =
        Loader::<i32, (usize, i32), (), _>::new(Batcher::new(2)).with_cache(MyCache::new());
    let v1 = loader.load(1);
    let v2 = loader.load(2);
    let all = future::try_join(v1, v2);
    assert_eq!(
        ((1, 10), (1, 20)),
        rt.block_on(all.boxed_local().compat()).unwrap(),
    );

    loader.clear();
    let v3 = loader.load(1);
    let v4 = loader.load(2);
    let all = future::try_join(v3, v4);
    assert_eq!(
        ((2, 10), (2, 20)),
        rt.block_on(all.boxed_local().compat()).unwrap(),
    );
}
