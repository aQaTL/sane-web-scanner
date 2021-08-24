<template>
	<div>
		<div>My blog</div>
		<NuxtLink to="/">Home</NuxtLink>
		<div>
			<div>Posts:</div>
			<div class="posts">
				<div v-for="post in posts" class="post">
					{{ post.name }}
				</div>
			</div>
		</div>
	</div>
</template>

<script lang="ts">
	import Vue from 'vue'

	export default Vue.extend({
		name: "blog",
		layout: "blog",

		head: function () {
			return {
				title: "Blog"
			};
		},

		data: function () {
			return {
				posts: [
					{
						name: "Ala"
					},
					{
						name: "Ma",
					},
					{
						name: "Kota"
					}
				],
			};
		},

	})
</script>

<style scoped>
	.post {
		border: 1px black solid;
	}
</style>