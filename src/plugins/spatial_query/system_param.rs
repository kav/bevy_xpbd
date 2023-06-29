use crate::prelude::*;
use bevy::{ecs::system::SystemParam, prelude::*, utils::HashMap};
use parry::query::{
    details::{
        RayCompositeShapeToiAndNormalBestFirstVisitor, TOICompositeShapeShapeBestFirstVisitor,
    },
    point::PointCompositeShapeProjBestFirstVisitor,
    visitors::RayIntersectionsVisitor,
};

#[cfg(feature = "2d")]
type ShapeRotation = Scalar;
#[cfg(feature = "3d")]
type ShapeRotation = Quat;

/// A system parameter for spatial queries that require more precise control.
///
/// ## Example
///
/// ```
/// use bevy::prelude::*;
/// # #[cfg(feature = "2d")]
/// # use bevy_xpbd_2d::prelude::*;
/// # #[cfg(feature = "3d")]
/// use bevy_xpbd_3d::prelude::*;
///
/// # #[cfg(all(feature = "3d", feature = "f32"))]
/// fn print_hits(spatial_query: SpatialQuery) {
///     let hits = spatial_query.ray_hits(
///         Vec3::ZERO,                    // Origin
///         Vec3::X,                       // Direction
///         100.0,                         // Maximum time of impact (travel distance)
///         20,                            // Maximum number of hits
///         true,                          // Does the ray treat colliders as "solid"
///         SpatialQueryFilter::default(), // Query filter
///     );
///
///     println!("{:?}", hits);
/// }
/// # #[cfg(not(all(feature = "3d", feature = "f32")))]
/// # fn print_hits() {}
/// ```
#[derive(SystemParam)]
pub struct SpatialQuery<'w, 's> {
    pub(crate) colliders: Query<
        'w,
        's,
        (
            Entity,
            &'static Position,
            &'static Rotation,
            &'static Collider,
            Option<&'static CollisionLayers>,
        ),
    >,
    /// The [`SpatialQueryPipeline`].
    pub query_pipeline: ResMut<'w, SpatialQueryPipeline>,
}

impl<'w, 's> SpatialQuery<'w, 's> {
    pub(crate) fn get_collider_hash_map(
        &self,
    ) -> HashMap<Entity, (Isometry<Scalar>, &dyn parry::shape::Shape, CollisionLayers)> {
        self.colliders
            .iter()
            .map(|(entity, position, rotation, collider, layers)| {
                (
                    entity,
                    (
                        utils::make_isometry(position.0, rotation),
                        &**collider.get_shape(),
                        layers.map_or(CollisionLayers::default(), |l| *l),
                    ),
                )
            })
            .collect()
    }
    /// Casts a [ray](RayCaster) from `origin` in a given `direction` and computes the closest [hit](RayHitData)
    /// with a collider. If there are no hits, `None` is returned.
    ///
    /// This should be used when you don't need frequent ray casts and want the result instantly.
    /// Otherwise, using [`RayCaster`] is recommended.
    pub fn cast_ray(
        &self,
        origin: Vector,
        direction: Vector,
        max_time_of_impact: f32,
        solid: bool,
        query_filter: SpatialQueryFilter,
    ) -> Option<RayHitData> {
        let colliders = self.get_collider_hash_map();
        let pipeline_shape = self
            .query_pipeline
            .as_composite_shape(&colliders, query_filter);
        let ray = parry::query::Ray::new(origin.into(), direction.into());
        let mut visitor = RayCompositeShapeToiAndNormalBestFirstVisitor::new(
            &pipeline_shape,
            &ray,
            max_time_of_impact,
            solid,
        );

        self.query_pipeline
            .qbvh
            .traverse_best_first(&mut visitor)
            .map(|(_, (entity_bits, hit))| RayHitData {
                entity: Entity::from_bits(entity_bits),
                time_of_impact: hit.toi,
                normal: hit.normal.into(),
            })
    }

    /// Casts a [ray](RayCaster) from `origin` in a given `direction` and computes all [hits](RayHitData)
    /// until `max_hits` is reached.
    ///
    /// Note that the order of the results is not guaranteed, and if there are more hits than `max_hits`, some hits will be missed.
    ///
    /// This should be used when you don't need frequent ray casts and want the result instantly.
    /// Otherwise, using [`RayCaster`] is recommended.
    pub fn ray_hits(
        &self,
        origin: Vector,
        direction: Vector,
        max_time_of_impact: f32,
        max_hits: u32,
        solid: bool,
        query_filter: SpatialQueryFilter,
    ) -> Vec<RayHitData> {
        let colliders = self.get_collider_hash_map();

        let mut hits = Vec::with_capacity(max_hits.min(100) as usize);
        let ray = parry::query::Ray::new(origin.into(), direction.into());

        let mut leaf_callback = &mut |entity_bits: &u64| {
            let entity = Entity::from_bits(*entity_bits);
            if let Some((iso, shape, layers)) = colliders.get(&entity) {
                if query_filter.test(entity, *layers) {
                    if let Some(hit) =
                        shape.cast_ray_and_get_normal(iso, &ray, max_time_of_impact, solid)
                    {
                        hits.push(RayHitData {
                            entity,
                            time_of_impact: hit.toi,
                            normal: hit.normal.into(),
                        });

                        return (hits.len() as u32) < max_hits;
                    }
                }
            }
            true
        };

        let mut visitor =
            RayIntersectionsVisitor::new(&ray, max_time_of_impact, &mut leaf_callback);
        self.query_pipeline.qbvh.traverse_depth_first(&mut visitor);

        hits
    }

    /// Casts a [ray](RayCaster) from `origin` in a given `direction` and computes all [hits](RayHitData)
    /// until the given `callback` returns false or all hits have been computed.
    ///
    /// Note that the order of the results is not guaranteed, and if there are more hits than `max_hits`, some hits will be missed.
    ///
    /// This should be used when you don't need frequent ray casts and want the result instantly.
    /// Otherwise, using [`RayCaster`] is recommended.
    pub fn ray_hits_callback(
        &self,
        origin: Vector,
        direction: Vector,
        max_time_of_impact: f32,
        solid: bool,
        query_filter: SpatialQueryFilter,
        mut callback: impl FnMut(Entity, RayHitData) -> bool,
    ) -> Vec<RayHitData> {
        let colliders = self.get_collider_hash_map();

        let mut hits = Vec::with_capacity(10);
        let ray = parry::query::Ray::new(origin.into(), direction.into());

        let mut leaf_callback = &mut |entity_bits: &u64| {
            let entity = Entity::from_bits(*entity_bits);
            if let Some((iso, shape, layers)) = colliders.get(&entity) {
                if query_filter.test(entity, *layers) {
                    if let Some(hit) =
                        shape.cast_ray_and_get_normal(iso, &ray, max_time_of_impact, solid)
                    {
                        let hit = RayHitData {
                            entity,
                            time_of_impact: hit.toi,
                            normal: hit.normal.into(),
                        };
                        hits.push(hit);

                        return callback(entity, hit);
                    }
                }
            }
            true
        };

        let mut visitor =
            RayIntersectionsVisitor::new(&ray, max_time_of_impact, &mut leaf_callback);
        self.query_pipeline.qbvh.traverse_depth_first(&mut visitor);

        hits
    }

    /// Casts a [shape](ShapeCaster) with a given rotation from `origin` in a given `direction` and computes the closest [hit](ShapeHit).
    /// with a collider. If there are no hits, `None` is returned.
    ///
    /// This should be used when you don't need to shape cast on every frame and want the result instantly.
    /// Otherwise, using [`ShapeCaster`] can be more convenient.
    #[allow(clippy::too_many_arguments)]
    pub fn cast_shape(
        &self,
        shape: &Collider,
        origin: Vector,
        shape_rotation: ShapeRotation,
        direction: Vector,
        max_time_of_impact: f32,
        ignore_origin_penetration: bool,
        query_filter: SpatialQueryFilter,
    ) -> Option<ShapeHitData> {
        let colliders = self.get_collider_hash_map();
        let rotation: Rotation;
        #[cfg(feature = "2d")]
        {
            rotation = Rotation::from_radians(shape_rotation);
        }
        #[cfg(feature = "3d")]
        {
            rotation = Rotation::from(shape_rotation);
        }

        let shape_isometry = utils::make_isometry(origin, &rotation);
        let shape_direction = direction.into();
        let pipeline_shape = self
            .query_pipeline
            .as_composite_shape(&colliders, query_filter);
        let mut visitor = TOICompositeShapeShapeBestFirstVisitor::new(
            &*self.query_pipeline.dispatcher,
            &shape_isometry,
            &shape_direction,
            &pipeline_shape,
            &**shape.get_shape(),
            max_time_of_impact,
            ignore_origin_penetration,
        );

        self.query_pipeline
            .qbvh
            .traverse_best_first(&mut visitor)
            .map(|(_, (entity_bits, hit))| ShapeHitData {
                entity: Entity::from_bits(entity_bits),
                time_of_impact: hit.toi,
                point1: hit.witness1.into(),
                point2: hit.witness2.into(),
                normal1: hit.normal1.into(),
                normal2: hit.normal2.into(),
            })
    }

    /// Finds the projection of a given point on the closest collider.
    pub fn project_point(
        &self,
        point: Vector,
        solid: bool,
        query_filter: SpatialQueryFilter,
    ) -> Option<PointProjection> {
        let point = point.into();
        let colliders = self.get_collider_hash_map();
        let pipeline_shape = self
            .query_pipeline
            .as_composite_shape(&colliders, query_filter);
        let mut visitor =
            PointCompositeShapeProjBestFirstVisitor::new(&pipeline_shape, &point, solid);

        self.query_pipeline
            .qbvh
            .traverse_best_first(&mut visitor)
            .map(|(_, (projection, entity_bits))| PointProjection {
                entity: Entity::from_bits(entity_bits),
                point: projection.point.into(),
                is_inside: projection.is_inside,
            })
    }
}

/// The result of a point projection on a collider.
pub struct PointProjection {
    /// The entity of the collider that the point was projected onto.
    pub entity: Entity,
    /// The point where the point was projected.
    pub point: Vector,
    /// True if the point was inside of the collider.
    pub is_inside: bool,
}
