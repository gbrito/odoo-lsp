use std::sync::RwLock;
use std::{collections::HashSet, fmt::Debug, hash::Hash, marker::PhantomData};

use dashmap::{DashMap, mapref::one::Ref};
use derive_more::{Deref, DerefMut};
use intmap::IntMap;
use smart_default::SmartDefault;

use crate::prelude::*;

use crate::{ImStr, format_loc};
use crate::{model::ModelName, record::Record};

use super::{_I, Symbol};

#[derive(SmartDefault, Deref)]
pub struct RecordIndex {
	#[deref]
	#[default(_code = "DashMap::with_shard_amount(4)")]
	inner: DashMap<RecordId, Record>,
	#[default(_code = "DashMap::with_shard_amount(4)")]
	by_model: DashMap<ModelName, HashSet<RecordId>>,
	#[default(_code = "DashMap::with_shard_amount(4)")]
	by_inherit_id: DashMap<RecordId, HashSet<RecordId>>,
	/// unqualified XML ID -> RecordID
	pub by_prefix: RwLock<RecordPrefixTrie>,
}

pub type RecordId = Symbol<Record>;
pub type RecordPrefixTrie = qp_trie::Trie<ImStr, HashSet<RecordId>>;

impl RecordIndex {
	pub fn insert(&self, qualified_id: RecordId, record: Record, prefix: Option<&mut RecordPrefixTrie>) {
		if self.inner.contains_key(&qualified_id) {
			return;
		}
		if let Some(model) = &record.model {
			self.by_model.entry(*model).or_default().insert(qualified_id);
		}
		if let Some(inherit_id) = &record.inherit_id {
			self.by_inherit_id.entry(*inherit_id).or_default().insert(qualified_id);
		}
		if let Some(prefix) = prefix {
			prefix
				.entry(record.id.clone())
				.or_insert_with(Default::default)
				.insert(qualified_id);
		} else if let Ok(mut by_prefix) = self.by_prefix.write() {
			by_prefix
				.entry(record.id.clone())
				.or_insert_with(Default::default)
				.insert(qualified_id);
		}
		self.inner.insert(qualified_id, record);
	}
	pub fn append(&self, prefix: Option<&mut RecordPrefixTrie>, records: impl IntoIterator<Item = Record>) {
		if let Some(prefix) = prefix {
			for record in records {
				let id = _I(record.qualified_id());
				self.insert(id.into(), record, Some(prefix));
			}
		} else {
			let mut prefix = self.by_prefix.write().expect(format_loc!("can't hold write lock now"));
			for record in records {
				let id = _I(record.qualified_id());
				self.insert(id.into(), record, Some(&mut prefix));
			}
		}
	}
	pub fn by_model(&self, model: &ModelName) -> impl Iterator<Item = Ref<'_, RecordId, Record>> {
		self.by_model
			.get(model)
			.into_iter()
			.flat_map(|ids| self.resolve_references(ids))
	}
	pub fn by_inherit_id(&self, inherit_id: &RecordId) -> impl Iterator<Item = Ref<'_, RecordId, Record>> {
		self.by_inherit_id
			.get(inherit_id)
			.into_iter()
			.flat_map(|ids| self.resolve_references(ids))
	}
	fn resolve_references<K>(
		&self,
		ids: Ref<K, HashSet<Symbol<Record>>>,
	) -> impl IntoIterator<Item = Ref<'_, RecordId, Record>>
	where
		K: PartialEq + Eq + Hash,
	{
		ids.value()
			.iter()
			.flat_map(|id| self.get(id).into_iter())
			.collect::<Vec<_>>()
	}
}

#[derive(Deref, DerefMut)]
pub struct SymbolMap<K, T = ()>(
	#[deref]
	#[deref_mut]
	IntMap<usize, T>,
	PhantomData<K>,
);

impl<K: Debug, T: Debug> Debug for SymbolMap<K, T> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_map().entries(self.0.iter()).finish()
	}
}

#[derive(Deref, DerefMut)]
pub struct SymbolSet<K>(pub(crate) SymbolMap<K, ()>);

impl<K, T> Default for SymbolMap<K, T> {
	#[inline]
	fn default() -> Self {
		Self(Default::default(), PhantomData)
	}
}

impl<K> Default for SymbolSet<K> {
	#[inline]
	fn default() -> Self {
		Self(Default::default())
	}
}

impl<K, T> SymbolMap<K, T> {
	#[inline]
	pub fn get(&self, key: &Symbol<K>) -> Option<&T> {
		self.0.get(key.into_usize())
	}
	#[inline]
	pub fn get_mut(&mut self, key: &Symbol<K>) -> Option<&mut T> {
		self.0.get_mut(key.into_usize())
	}
	pub fn keys(&self) -> impl Iterator<Item = Symbol<K>> + '_ {
		self.0.iter().map(|(key, _)| Spur::try_from_usize(key).unwrap().into())
	}
	pub fn iter(&self) -> IterMap<'_, impl Iterator<Item = (usize, &T)>, K, T> {
		IterMap(self.0.iter(), PhantomData)
	}
}

impl<K> SymbolSet<K> {
	#[inline]
	pub fn insert(&mut self, key: Symbol<K>) -> bool {
		self.0.0.insert_checked(key.into_usize(), ())
	}
	#[inline]
	pub fn contains_key(&self, key: Symbol<K>) -> bool {
		self.0.0.contains_key(key.into_usize() as _)
	}
	pub fn iter(&self) -> IterSet<'_, impl Iterator<Item = (usize, &())>, K> {
		IterSet(self.0.0.iter(), PhantomData)
	}
	pub fn extend<I>(&mut self, items: I)
	where
		I: IntoIterator<Item = Symbol<K>>,
	{
		(self.0.0).extend(items.into_iter().map(|key| (key.into_usize(), ())));
	}
}

pub struct IterMap<'a, I, K, T>(I, PhantomData<(&'a K, &'a T)>);

impl<'iter, K, T, I> Iterator for IterMap<'iter, I, K, T>
where
	I: Iterator<Item = (usize, &'iter T)>,
{
	type Item = (Symbol<T>, &'iter T);

	fn next(&mut self) -> Option<Self::Item> {
		let (next_key, next_value) = self.0.next()?;
		Some((Symbol::from(Spur::try_from_usize(next_key as _).unwrap()), next_value))
	}
}

pub struct IterSet<'a, I, T>(I, PhantomData<&'a T>);

impl<'iter, T, I> Iterator for IterSet<'iter, I, T>
where
	I: Iterator<Item = (usize, &'iter ())>,
{
	type Item = Symbol<T>;

	fn next(&mut self) -> Option<Self::Item> {
		let (next, ()) = self.0.next()?;
		Some(Symbol::from(Spur::try_from_usize(next as _).unwrap()))
	}
}
